/// Cluster management module for distributed MerlionOS instances.
///
/// Provides node discovery, heartbeat-based health monitoring, leader
/// election, and remote command execution across a cluster of MerlionOS
/// nodes communicating over UDP.
///
/// # Architecture
///
/// Each node maintains a [`ClusterState`] protected by a spin mutex. Nodes
/// announce themselves via [`join`], discover peers with [`discover`]
/// broadcast packets, and keep membership current through periodic
/// [`heartbeat`] messages. A simple leader election scheme selects the
/// online node with the highest ID as the cluster leader.
///
/// # Wire protocol
///
/// Cluster control messages are sent as UDP datagrams on [`CLUSTER_PORT`].
/// Each message starts with a single-byte tag followed by payload fields:
///
/// ```text
/// JOIN:      [0x01][node_id:8][port:2][name_len:4][name]
/// HEARTBEAT: [0x02][node_id:8][tick:8]
/// DISCOVER:  [0x03][node_id:8][port:2]
/// DISCOVER_REPLY: [0x04][node_id:8][port:2][name_len:4][name]
/// ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::{netstack, rpc, serial_println, timer};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// UDP port used for cluster control messages.
pub const CLUSTER_PORT: u16 = 9200;

/// Number of ticks before a node is considered offline.
/// At 100 Hz PIT, 500 ticks = 5 seconds.
const HEARTBEAT_TIMEOUT_TICKS: u64 = 500;

/// Broadcast IPv4 address used for discovery.
const BROADCAST_IP: [u8; 4] = [255, 255, 255, 255];

/// Message tag bytes.
const MSG_JOIN: u8 = 0x01;
const MSG_HEARTBEAT: u8 = 0x02;
const MSG_DISCOVER: u8 = 0x03;
const MSG_DISCOVER_REPLY: u8 = 0x04;

/// Monotonically increasing node ID generator for this instance.
static LOCAL_NODE_ID: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// NodeStatus
// ---------------------------------------------------------------------------

/// Operational status of a node in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Node is reachable and sending heartbeats.
    Online,
    /// Node has not responded within the heartbeat timeout.
    Offline,
    /// Node has announced itself but has not yet completed its first
    /// heartbeat cycle.
    Joining,
}

impl core::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            NodeStatus::Online => write!(f, "Online"),
            NodeStatus::Offline => write!(f, "Offline"),
            NodeStatus::Joining => write!(f, "Joining"),
        }
    }
}

// ---------------------------------------------------------------------------
// NodeInfo
// ---------------------------------------------------------------------------

/// Metadata for a single node in the cluster.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Unique numeric identifier for this node.
    pub id: u64,
    /// IPv4 address as four octets.
    pub ip: [u8; 4],
    /// UDP port the node listens on for RPC / cluster traffic.
    pub port: u16,
    /// Human-readable name (e.g. `"merlion-node-1"`).
    pub name: String,
    /// Current operational status.
    pub status: NodeStatus,
    /// PIT tick value when we last heard from this node.
    pub last_seen_tick: u64,
}

impl NodeInfo {
    /// Returns `true` if the node has exceeded the heartbeat timeout.
    pub fn is_expired(&self, current_tick: u64) -> bool {
        current_tick.saturating_sub(self.last_seen_tick) > HEARTBEAT_TIMEOUT_TICKS
    }

    /// Format the node's IP address as a dotted-quad string.
    pub fn ip_str(&self) -> String {
        format!("{}.{}.{}.{}", self.ip[0], self.ip[1], self.ip[2], self.ip[3])
    }
}

// ---------------------------------------------------------------------------
// ClusterState
// ---------------------------------------------------------------------------

/// Global cluster membership state, protected by a spin mutex.
pub struct ClusterState {
    /// Known nodes (including ourselves).
    pub nodes: Mutex<Vec<NodeInfo>>,
}

/// The singleton cluster state instance.
pub static CLUSTER: ClusterState = ClusterState {
    nodes: Mutex::new(Vec::new()),
};

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the cluster subsystem with a node ID derived from our IP
/// address and the current tick counter to ensure uniqueness.
pub fn init() {
    let ip = crate::net::NET.lock().ip.0;
    let id = u64::from(ip[0]) << 24
        | u64::from(ip[1]) << 16
        | u64::from(ip[2]) << 8
        | u64::from(ip[3])
        | (timer::ticks() << 32);
    LOCAL_NODE_ID.store(id, Ordering::SeqCst);
    serial_println!("[cluster] init: local node id = {:#x}", id);
}

/// Return the local node's unique ID.
pub fn local_id() -> u64 {
    LOCAL_NODE_ID.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// join — announce self to the cluster
// ---------------------------------------------------------------------------

/// Announce this node to the cluster by broadcasting a JOIN message.
///
/// Registers ourselves in the local node list and sends a UDP broadcast
/// so that existing members can add us to their state.
pub fn join(ip: [u8; 4], port: u16, name: &str) -> bool {
    let id = local_id();
    let now = timer::ticks();

    // Insert ourselves into local state.
    {
        let mut nodes = CLUSTER.nodes.lock();
        // Avoid duplicates.
        if !nodes.iter().any(|n| n.id == id) {
            nodes.push(NodeInfo {
                id,
                ip,
                port,
                name: String::from(name),
                status: NodeStatus::Online,
                last_seen_tick: now,
            });
        }
    }

    // Build and broadcast the JOIN message.
    let mut payload = Vec::with_capacity(1 + 8 + 2 + 4 + name.len());
    payload.push(MSG_JOIN);
    payload.extend_from_slice(&id.to_be_bytes());
    payload.extend_from_slice(&port.to_be_bytes());
    let name_bytes = name.as_bytes();
    payload.extend_from_slice(&(name_bytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(name_bytes);

    serial_println!("[cluster] join: broadcasting as '{}' id={:#x}", name, id);
    netstack::send_udp(BROADCAST_IP, CLUSTER_PORT, CLUSTER_PORT, &payload)
}

// ---------------------------------------------------------------------------
// discover — broadcast discovery packet
// ---------------------------------------------------------------------------

/// Broadcast a DISCOVER packet to find other MerlionOS nodes on the
/// network.
///
/// Nodes that receive this packet should respond with a DISCOVER_REPLY
/// containing their own identity, which is handled by [`handle_message`].
pub fn discover() -> bool {
    let id = local_id();
    let port = CLUSTER_PORT;

    let mut payload = Vec::with_capacity(1 + 8 + 2);
    payload.push(MSG_DISCOVER);
    payload.extend_from_slice(&id.to_be_bytes());
    payload.extend_from_slice(&port.to_be_bytes());

    serial_println!("[cluster] discover: broadcasting probe");
    netstack::send_udp(BROADCAST_IP, CLUSTER_PORT, CLUSTER_PORT, &payload)
}

// ---------------------------------------------------------------------------
// heartbeat — send periodic alive messages
// ---------------------------------------------------------------------------

/// Send a heartbeat message to all known peers and expire nodes that have
/// not responded within [`HEARTBEAT_TIMEOUT_TICKS`].
///
/// Should be called periodically from a timer-driven task (e.g. every
/// 100 ticks / 1 second).
pub fn heartbeat() {
    let id = local_id();
    let now = timer::ticks();

    // Build heartbeat payload.
    let mut payload = Vec::with_capacity(1 + 8 + 8);
    payload.push(MSG_HEARTBEAT);
    payload.extend_from_slice(&id.to_be_bytes());
    payload.extend_from_slice(&now.to_be_bytes());

    // Send to every known peer and update stale nodes.
    let mut nodes = CLUSTER.nodes.lock();
    for node in nodes.iter_mut() {
        if node.id == id {
            // Refresh our own timestamp.
            node.last_seen_tick = now;
            node.status = NodeStatus::Online;
            continue;
        }

        // Send heartbeat to this peer.
        let _ = netstack::send_udp(node.ip, CLUSTER_PORT, CLUSTER_PORT, &payload);

        // Mark stale nodes as offline.
        if node.is_expired(now) && node.status != NodeStatus::Offline {
            serial_println!(
                "[cluster] heartbeat: node {} ({}) timed out",
                node.id, node.name
            );
            node.status = NodeStatus::Offline;
        }
    }
}

// ---------------------------------------------------------------------------
// handle_message — process incoming cluster messages
// ---------------------------------------------------------------------------

/// Process a cluster control message received from `from_ip`.
///
/// Dispatches on the leading tag byte to handle JOIN, HEARTBEAT,
/// DISCOVER, and DISCOVER_REPLY messages, updating the local cluster
/// state accordingly.
pub fn handle_message(from_ip: [u8; 4], data: &[u8]) {
    if data.is_empty() {
        return;
    }

    match data[0] {
        MSG_JOIN => handle_join(from_ip, &data[1..]),
        MSG_HEARTBEAT => handle_heartbeat(from_ip, &data[1..]),
        MSG_DISCOVER => handle_discover(from_ip, &data[1..]),
        MSG_DISCOVER_REPLY => handle_discover_reply(from_ip, &data[1..]),
        tag => {
            serial_println!("[cluster] handle_message: unknown tag {:#x}", tag);
        }
    }
}

/// Process an incoming JOIN message, adding the sender to local state.
fn handle_join(from_ip: [u8; 4], body: &[u8]) {
    if body.len() < 8 + 2 + 4 {
        return;
    }
    let node_id = u64::from_be_bytes([
        body[0], body[1], body[2], body[3],
        body[4], body[5], body[6], body[7],
    ]);
    let port = u16::from_be_bytes([body[8], body[9]]);
    let name_len = u32::from_be_bytes([body[10], body[11], body[12], body[13]]) as usize;
    if body.len() < 14 + name_len {
        return;
    }
    let name = core::str::from_utf8(&body[14..14 + name_len]).unwrap_or("unknown");

    upsert_node(node_id, from_ip, port, name, NodeStatus::Joining);
    serial_println!("[cluster] join received: '{}' id={:#x}", name, node_id);
}

/// Process an incoming HEARTBEAT, refreshing the sender's last-seen tick.
fn handle_heartbeat(from_ip: [u8; 4], body: &[u8]) {
    if body.len() < 8 + 8 {
        return;
    }
    let node_id = u64::from_be_bytes([
        body[0], body[1], body[2], body[3],
        body[4], body[5], body[6], body[7],
    ]);

    let now = timer::ticks();
    let mut nodes = CLUSTER.nodes.lock();
    for node in nodes.iter_mut() {
        if node.id == node_id {
            node.last_seen_tick = now;
            if node.status != NodeStatus::Online {
                serial_println!("[cluster] node {} is now Online", node_id);
            }
            node.status = NodeStatus::Online;
            return;
        }
    }

    // Unknown sender — add as Joining.
    nodes.push(NodeInfo {
        id: node_id,
        ip: from_ip,
        port: CLUSTER_PORT,
        name: format!("node-{:#x}", node_id),
        status: NodeStatus::Joining,
        last_seen_tick: now,
    });
}

/// Respond to a DISCOVER probe with our own identity.
fn handle_discover(from_ip: [u8; 4], body: &[u8]) {
    if body.len() < 8 + 2 {
        return;
    }

    let id = local_id();
    let our_name = {
        let nodes = CLUSTER.nodes.lock();
        nodes.iter()
            .find(|n| n.id == id)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| String::from("merlion"))
    };

    let mut reply = Vec::with_capacity(1 + 8 + 2 + 4 + our_name.len());
    reply.push(MSG_DISCOVER_REPLY);
    reply.extend_from_slice(&id.to_be_bytes());
    reply.extend_from_slice(&CLUSTER_PORT.to_be_bytes());
    let nb = our_name.as_bytes();
    reply.extend_from_slice(&(nb.len() as u32).to_be_bytes());
    reply.extend_from_slice(nb);

    let _ = netstack::send_udp(from_ip, CLUSTER_PORT, CLUSTER_PORT, &reply);
}

/// Process a DISCOVER_REPLY, adding the responder to local state.
fn handle_discover_reply(from_ip: [u8; 4], body: &[u8]) {
    if body.len() < 8 + 2 + 4 {
        return;
    }
    let node_id = u64::from_be_bytes([
        body[0], body[1], body[2], body[3],
        body[4], body[5], body[6], body[7],
    ]);
    let port = u16::from_be_bytes([body[8], body[9]]);
    let name_len = u32::from_be_bytes([body[10], body[11], body[12], body[13]]) as usize;
    if body.len() < 14 + name_len {
        return;
    }
    let name = core::str::from_utf8(&body[14..14 + name_len]).unwrap_or("unknown");

    upsert_node(node_id, from_ip, port, name, NodeStatus::Online);
    serial_println!("[cluster] discovered: '{}' id={:#x}", name, node_id);
}

// ---------------------------------------------------------------------------
// remote_exec — execute a command on a remote node
// ---------------------------------------------------------------------------

/// Execute `command` on a remote node identified by `node_id`.
///
/// Looks up the node's IP address in the cluster state and delegates to
/// [`crate::rpc::remote_exec`]. Returns the RPC request ID on success,
/// or an error string if the node is unknown or the send fails.
pub fn remote_exec(node_id: u64, command: &str) -> Result<u64, &'static str> {
    let (ip, port) = {
        let nodes = CLUSTER.nodes.lock();
        let node = nodes.iter().find(|n| n.id == node_id);
        match node {
            Some(n) if n.status == NodeStatus::Online => (n.ip, n.port),
            Some(_) => return Err("cluster: target node is not online"),
            None => return Err("cluster: unknown node id"),
        }
    };

    serial_println!(
        "[cluster] remote_exec: node {:#x} cmd='{}'",
        node_id, command
    );
    rpc::remote_exec(ip, port, command)
}

// ---------------------------------------------------------------------------
// cluster_status — formatted display of all nodes
// ---------------------------------------------------------------------------

/// Return a human-readable summary of all nodes in the cluster.
///
/// The output includes each node's ID, IP, name, status, and the tick
/// when it was last seen. Suitable for display in the kernel shell.
pub fn cluster_status() -> String {
    let nodes = CLUSTER.nodes.lock();
    if nodes.is_empty() {
        return String::from("Cluster: no nodes registered\n");
    }

    let leader_id = find_leader_id(&nodes);
    let mut out = String::from("Cluster status:\n");
    out.push_str("  ID               IP              Name            Status   Last Seen  Leader\n");
    out.push_str("  ----             --              ----            ------   ---------  ------\n");

    for node in nodes.iter() {
        let is_leader = Some(node.id) == leader_id;
        let line = format!(
            "  {:<16x} {:<15} {:<15} {:<8} {:<10} {}\n",
            node.id,
            node.ip_str(),
            node.name,
            node.status,
            node.last_seen_tick,
            if is_leader { "*" } else { "" },
        );
        out.push_str(&line);
    }

    let total = nodes.len();
    let online = nodes.iter().filter(|n| n.status == NodeStatus::Online).count();
    out.push_str(&format!("  ({} nodes, {} online)\n", total, online));
    out
}

// ---------------------------------------------------------------------------
// elect_leader — simple leader election (highest node ID)
// ---------------------------------------------------------------------------

/// Elect a cluster leader using a simple highest-ID-wins scheme.
///
/// Only nodes with [`NodeStatus::Online`] are eligible. Returns the
/// elected leader's [`NodeInfo`] (cloned), or `None` if no online nodes
/// exist.
pub fn elect_leader() -> Option<NodeInfo> {
    let nodes = CLUSTER.nodes.lock();
    let leader = nodes.iter()
        .filter(|n| n.status == NodeStatus::Online)
        .max_by_key(|n| n.id)?;

    serial_println!(
        "[cluster] leader elected: '{}' id={:#x}",
        leader.name, leader.id
    );
    Some(leader.clone())
}

/// Find the leader's ID without cloning the full NodeInfo.
fn find_leader_id(nodes: &[NodeInfo]) -> Option<u64> {
    nodes.iter()
        .filter(|n| n.status == NodeStatus::Online)
        .max_by_key(|n| n.id)
        .map(|n| n.id)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Insert a new node or update an existing one in the cluster state.
fn upsert_node(id: u64, ip: [u8; 4], port: u16, name: &str, status: NodeStatus) {
    let now = timer::ticks();
    let mut nodes = CLUSTER.nodes.lock();

    for node in nodes.iter_mut() {
        if node.id == id {
            node.ip = ip;
            node.port = port;
            node.name = String::from(name);
            node.last_seen_tick = now;
            // Only upgrade status, never downgrade from Online.
            if node.status != NodeStatus::Online {
                node.status = status;
            }
            return;
        }
    }

    nodes.push(NodeInfo {
        id,
        ip,
        port,
        name: String::from(name),
        status,
        last_seen_tick: now,
    });
}
