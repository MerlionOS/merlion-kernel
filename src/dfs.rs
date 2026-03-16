/// Distributed filesystem for MerlionOS.
/// Network file sharing across multiple nodes with simplified Raft consensus
/// for data replication and consistency.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum nodes in the cluster.
const MAX_NODES: usize = 32;

/// Maximum mount points.
const MAX_MOUNTS: usize = 16;

/// Maximum Raft log entries retained.
const MAX_LOG_ENTRIES: usize = 256;

/// Role a node plays in the Raft cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    Leader,
    Follower,
    Candidate,
}

/// Connectivity state of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Online,
    Offline,
    Syncing,
}

/// A single node in the distributed filesystem cluster.
#[derive(Debug, Clone)]
pub struct DfsNode {
    pub id: u32,
    pub ip: [u8; 4],
    pub port: u16,
    pub role: NodeRole,
    pub state: NodeState,
    pub last_heartbeat: u64,
    pub files_hosted: usize,
}

/// Operation recorded in the Raft log.
#[derive(Debug, Clone)]
pub enum DfsOperation {
    CreateFile(String, Vec<u8>),
    DeleteFile(String),
    UpdateFile(String, Vec<u8>),
}

/// A single entry in the Raft replicated log.
#[derive(Debug, Clone)]
struct RaftLogEntry {
    term: u64,
    index: u64,
    operation: DfsOperation,
}

/// Simplified Raft consensus state.
struct RaftState {
    current_term: u64,
    voted_for: Option<u32>,
    leader_id: Option<u32>,
    commit_index: u64,
    log: Vec<RaftLogEntry>,
}

impl RaftState {
    fn new() -> Self {
        Self {
            current_term: 1,
            voted_for: None,
            leader_id: None,
            commit_index: 0,
            log: Vec::new(),
        }
    }

    /// Append an operation to the log and return its index.
    fn append(&mut self, op: DfsOperation) -> u64 {
        let index = self.log.len() as u64 + 1;
        self.log.push(RaftLogEntry {
            term: self.current_term,
            index,
            operation: op,
        });
        if self.log.len() > MAX_LOG_ENTRIES {
            self.log.remove(0);
        }
        self.commit_index = index;
        index
    }
}

/// A remote filesystem mount point.
#[derive(Debug, Clone)]
pub struct DfsMount {
    pub node_ip: [u8; 4],
    pub remote_path: String,
    pub local_mount: String,
    pub connected: bool,
}

/// Internal state of the distributed filesystem.
struct DfsInner {
    nodes: Vec<DfsNode>,
    mounts: Vec<DfsMount>,
    raft: RaftState,
    local_node_id: u32,
    next_node_id: u32,
}

impl DfsInner {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            mounts: Vec::new(),
            raft: RaftState::new(),
            local_node_id: 0,
            next_node_id: 1,
        }
    }
}

/// Global DFS state, protected by a spinlock.
static DFS: Mutex<Option<DfsInner>> = Mutex::new(None);

/// Atomic counters for DFS operations.
static FILES_SYNCED: AtomicU64 = AtomicU64::new(0);
static OPS_COMMITTED: AtomicU64 = AtomicU64::new(0);
static HEARTBEATS_SENT: AtomicU64 = AtomicU64::new(0);

fn fmt_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

// ---------------------------------------------------------------------------
// Node management
// ---------------------------------------------------------------------------

/// Add a node to the cluster. Returns the assigned node id, or `None` if
/// full or uninitialised.
pub fn add_node(id: u32, ip: [u8; 4], port: u16) -> Option<u32> {
    let mut dfs = DFS.lock();
    let inner = dfs.as_mut()?;
    if inner.nodes.len() >= MAX_NODES {
        return None;
    }
    if inner.nodes.iter().any(|n| n.id == id) {
        return None;
    }
    inner.nodes.push(DfsNode {
        id,
        ip,
        port,
        role: NodeRole::Follower,
        state: NodeState::Online,
        last_heartbeat: 0,
        files_hosted: 0,
    });
    Some(id)
}

/// Remove a node from the cluster by id. Returns `true` if found.
pub fn remove_node(id: u32) -> bool {
    let mut dfs = DFS.lock();
    let inner = match dfs.as_mut() {
        Some(i) => i,
        None => return false,
    };
    if let Some(pos) = inner.nodes.iter().position(|n| n.id == id) {
        inner.nodes.remove(pos);
        true
    } else {
        false
    }
}

/// Return a formatted list of all nodes in the cluster.
pub fn list_nodes() -> String {
    let dfs = DFS.lock();
    let inner = match dfs.as_ref() {
        Some(i) => i,
        None => return "(dfs not initialised)\n".into(),
    };
    if inner.nodes.is_empty() {
        return "(no nodes in cluster)\n".into();
    }
    let mut out = String::new();
    out.push_str("ID   IP               PORT  ROLE       STATE    FILES  HEARTBEAT\n");
    out.push_str("---- ---------------- ----- ---------- -------- ------ ---------\n");
    for n in &inner.nodes {
        let role = match n.role {
            NodeRole::Leader    => "Leader    ",
            NodeRole::Follower  => "Follower  ",
            NodeRole::Candidate => "Candidate ",
        };
        let state = match n.state {
            NodeState::Online  => "Online  ",
            NodeState::Offline => "Offline ",
            NodeState::Syncing => "Syncing ",
        };
        out.push_str(&format!(
            "{:<4} {:<16} {:<5} {} {} {:<6} {}\n",
            n.id, fmt_ip(n.ip), n.port, role, state,
            n.files_hosted, n.last_heartbeat,
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Raft consensus helpers
// ---------------------------------------------------------------------------

/// Run a simplified leader election. The online node with the lowest id
/// becomes the leader; all others become followers.
pub fn elect_leader() {
    let mut dfs = DFS.lock();
    let inner = match dfs.as_mut() {
        Some(i) => i,
        None => return,
    };
    inner.raft.current_term += 1;

    // Find online node with lowest id.
    let winner = inner.nodes.iter()
        .filter(|n| n.state == NodeState::Online)
        .min_by_key(|n| n.id)
        .map(|n| n.id);

    inner.raft.leader_id = winner;
    inner.raft.voted_for = winner;

    if let Some(leader_id) = winner {
        for node in &mut inner.nodes {
            if node.id == leader_id {
                node.role = NodeRole::Leader;
            } else {
                node.role = NodeRole::Follower;
            }
        }
    }
}

/// Send a heartbeat to all nodes (simulated). Marks offline any node
/// whose last heartbeat was more than `timeout` ticks ago relative to
/// `current_tick`.
pub fn heartbeat(current_tick: u64, timeout: u64) {
    let mut dfs = DFS.lock();
    let inner = match dfs.as_mut() {
        Some(i) => i,
        None => return,
    };
    for node in &mut inner.nodes {
        if node.id == inner.local_node_id {
            node.last_heartbeat = current_tick;
            node.state = NodeState::Online;
            continue;
        }
        if current_tick.saturating_sub(node.last_heartbeat) > timeout {
            node.state = NodeState::Offline;
        }
    }
    HEARTBEATS_SENT.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Mount / remote file operations
// ---------------------------------------------------------------------------

/// Mount a remote filesystem at a local path. Returns `Ok(())` or an error
/// if the mount table is full or DFS is not initialised.
pub fn mount(node_ip: [u8; 4], remote_path: &str, local_mount: &str) -> Result<(), &'static str> {
    let mut dfs = DFS.lock();
    let inner = dfs.as_mut().ok_or("dfs not initialised")?;
    if inner.mounts.len() >= MAX_MOUNTS {
        return Err("mount table full");
    }
    if inner.mounts.iter().any(|m| m.local_mount == local_mount) {
        return Err("mount point already in use");
    }
    inner.mounts.push(DfsMount {
        node_ip,
        remote_path: String::from(remote_path),
        local_mount: String::from(local_mount),
        connected: true,
    });
    Ok(())
}

/// Unmount a previously mounted remote filesystem.
pub fn unmount(mount_point: &str) -> Result<(), &'static str> {
    let mut dfs = DFS.lock();
    let inner = dfs.as_mut().ok_or("dfs not initialised")?;
    if let Some(pos) = inner.mounts.iter().position(|m| m.local_mount == mount_point) {
        inner.mounts.remove(pos);
        Ok(())
    } else {
        Err("mount point not found")
    }
}

/// Return a formatted list of all mount points.
pub fn list_mounts() -> String {
    let dfs = DFS.lock();
    let inner = match dfs.as_ref() {
        Some(i) => i,
        None => return "(dfs not initialised)\n".into(),
    };
    if inner.mounts.is_empty() {
        return "(no mounts)\n".into();
    }
    let mut out = String::new();
    out.push_str("REMOTE_IP        REMOTE_PATH          LOCAL_MOUNT          STATUS\n");
    out.push_str("---------------- -------------------- -------------------- ------\n");
    for m in &inner.mounts {
        let status = if m.connected { "OK" } else { "DISC" };
        out.push_str(&format!(
            "{:<16} {:<20} {:<20} {}\n",
            fmt_ip(m.node_ip), m.remote_path, m.local_mount, status,
        ));
    }
    out
}

/// Read a file from a remote mount (simulated — returns placeholder content).
pub fn remote_read(mount_point: &str, path: &str) -> Result<String, &'static str> {
    let dfs = DFS.lock();
    let inner = dfs.as_ref().ok_or("dfs not initialised")?;
    let mnt = inner.mounts.iter()
        .find(|m| m.local_mount == mount_point)
        .ok_or("mount point not found")?;
    if !mnt.connected {
        return Err("mount disconnected");
    }
    Ok(format!("[remote:{}:{}{}]", fmt_ip(mnt.node_ip), mnt.remote_path, path))
}

/// Write data to a file on a remote mount (simulated — logs the operation
/// via Raft consensus).
pub fn remote_write(mount_point: &str, path: &str, data: &[u8]) -> Result<(), &'static str> {
    let mut dfs = DFS.lock();
    let inner = dfs.as_mut().ok_or("dfs not initialised")?;
    let mnt = inner.mounts.iter()
        .find(|m| m.local_mount == mount_point)
        .ok_or("mount point not found")?;
    if !mnt.connected {
        return Err("mount disconnected");
    }
    let full_path = format!("{}{}", mnt.remote_path, path);
    inner.raft.append(DfsOperation::UpdateFile(full_path, data.to_vec()));
    OPS_COMMITTED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Replicate pending log entries to follower nodes (simulated).
pub fn sync_files() {
    let mut dfs = DFS.lock();
    let inner = match dfs.as_mut() {
        Some(i) => i,
        None => return,
    };
    let follower_count = inner.nodes.iter()
        .filter(|n| n.role == NodeRole::Follower && n.state == NodeState::Online)
        .count();
    // Mark syncing followers, then back to online.
    for node in &mut inner.nodes {
        if node.role == NodeRole::Follower && node.state == NodeState::Online {
            node.state = NodeState::Syncing;
            node.files_hosted = inner.raft.commit_index as usize;
            node.state = NodeState::Online;
        }
    }
    FILES_SYNCED.fetch_add(follower_count as u64, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Info / stats
// ---------------------------------------------------------------------------

/// Return a summary of the distributed filesystem.
pub fn dfs_info() -> String {
    let dfs = DFS.lock();
    let inner = match dfs.as_ref() {
        Some(i) => i,
        None => return "(dfs not initialised)\n".into(),
    };
    let leader = inner.nodes.iter()
        .find(|n| n.role == NodeRole::Leader)
        .map(|n| format!("node {} ({})", n.id, fmt_ip(n.ip)))
        .unwrap_or_else(|| "(none)".into());
    let online = inner.nodes.iter().filter(|n| n.state == NodeState::Online).count();
    format!(
        "DFS cluster info:\n  Nodes: {} total, {} online\n  Leader: {}\n  Raft term: {}\n  \
         Commit index: {}\n  Log entries: {}\n  Mounts: {}\n",
        inner.nodes.len(), online, leader,
        inner.raft.current_term, inner.raft.commit_index,
        inner.raft.log.len(), inner.mounts.len(),
    )
}

/// Return operational statistics as a formatted string.
pub fn dfs_stats() -> String {
    let synced = FILES_SYNCED.load(Ordering::Relaxed);
    let committed = OPS_COMMITTED.load(Ordering::Relaxed);
    let heartbeats = HEARTBEATS_SENT.load(Ordering::Relaxed);
    format!(
        "DFS statistics:\n  Files synced: {}\n  Ops committed: {}\n  Heartbeats sent: {}\n",
        synced, committed, heartbeats,
    )
}

/// Initialise the distributed filesystem with the local node as leader.
pub fn init() {
    let mut inner = DfsInner::new();
    let local_id = inner.next_node_id;
    inner.next_node_id += 1;
    inner.local_node_id = local_id;
    inner.nodes.push(DfsNode {
        id: local_id,
        ip: [127, 0, 0, 1],
        port: 9000,
        role: NodeRole::Leader,
        state: NodeState::Online,
        last_heartbeat: 0,
        files_hosted: 0,
    });
    inner.raft.leader_id = Some(local_id);
    *DFS.lock() = Some(inner);
}
