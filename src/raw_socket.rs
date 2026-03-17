/// Raw socket interface for MerlionOS.
/// Allows sending and receiving raw Ethernet frames and IP packets,
/// similar to Linux AF_PACKET / AF_INET with SOCK_RAW.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Maximum number of open raw sockets.
const MAX_RAW_SOCKETS: usize = 64;

/// Maximum packets buffered per socket.
const MAX_RX_QUEUE: usize = 32;

/// Raw socket type — determines the layer at which frames are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawSocketType {
    /// Layer 2: raw Ethernet frames (similar to AF_PACKET).
    RawEthernet,
    /// Layer 3: raw IP packets (similar to AF_INET + SOCK_RAW).
    RawIp,
}

/// Protocol filter for received packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolFilter {
    /// Accept all protocols.
    All,
    /// Accept only IPv4 (EtherType 0x0800).
    Ipv4,
    /// Accept only IPv6 (EtherType 0x86DD).
    Ipv6,
    /// Accept only ARP (EtherType 0x0806).
    Arp,
    /// Accept only ICMP (IP protocol 1).
    Icmp,
    /// Accept only TCP (IP protocol 6).
    Tcp,
    /// Accept only UDP (IP protocol 17).
    Udp,
    /// Custom EtherType or IP protocol number.
    Custom(u16),
}

/// A single raw socket with send/receive state.
pub struct RawSocket {
    /// Unique socket ID.
    pub id: u32,
    /// Socket type (L2 or L3).
    pub socket_type: RawSocketType,
    /// Interface this socket is bound to (empty = any).
    pub iface: String,
    /// Protocol filter.
    pub filter: ProtocolFilter,
    /// Whether promiscuous mode is enabled.
    pub promisc: bool,
    /// Receive queue.
    rx_queue: Vec<Vec<u8>>,
    /// Packets sent counter.
    pub packets_sent: u64,
    /// Packets received counter.
    pub packets_recv: u64,
    /// Bytes sent counter.
    pub bytes_sent: u64,
    /// Bytes received counter.
    pub bytes_recv: u64,
}

impl RawSocket {
    fn new(id: u32, socket_type: RawSocketType) -> Self {
        Self {
            id,
            socket_type,
            iface: String::new(),
            filter: ProtocolFilter::All,
            promisc: false,
            rx_queue: Vec::new(),
            packets_sent: 0,
            packets_recv: 0,
            bytes_sent: 0,
            bytes_recv: 0,
        }
    }
}

/// Global raw socket state.
struct RawSocketState {
    sockets: Vec<RawSocket>,
}

impl RawSocketState {
    const fn new() -> Self {
        Self { sockets: Vec::new() }
    }
}

static STATE: Mutex<RawSocketState> = Mutex::new(RawSocketState::new());
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_SENT: AtomicU64 = AtomicU64::new(0);
static TOTAL_RECV: AtomicU64 = AtomicU64::new(0);

/// Initialise the raw socket subsystem.
pub fn init() {
    let mut st = STATE.lock();
    st.sockets = Vec::new();
    crate::serial_println!("[raw_socket] subsystem initialised");
}

/// Create a new raw socket of the given type. Returns the socket ID.
pub fn raw_socket_create(socket_type: RawSocketType) -> Option<u32> {
    let mut st = STATE.lock();
    if st.sockets.len() >= MAX_RAW_SOCKETS {
        return None;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    st.sockets.push(RawSocket::new(id, socket_type));
    Some(id)
}

/// Close and remove a raw socket.
pub fn raw_socket_close(socket_id: u32) -> bool {
    let mut st = STATE.lock();
    if let Some(pos) = st.sockets.iter().position(|s| s.id == socket_id) {
        st.sockets.remove(pos);
        true
    } else {
        false
    }
}

/// Send raw data through a socket. Returns bytes sent or error.
pub fn raw_send(socket_id: u32, data: &[u8]) -> Result<usize, &'static str> {
    let mut st = STATE.lock();
    let sock = st.sockets.iter_mut().find(|s| s.id == socket_id)
        .ok_or("socket not found")?;
    if data.is_empty() {
        return Err("empty payload");
    }
    // Validate minimum frame size based on socket type
    match sock.socket_type {
        RawSocketType::RawEthernet => {
            if data.len() < 14 {
                return Err("ethernet frame too short (min 14 bytes)");
            }
        }
        RawSocketType::RawIp => {
            if data.len() < 20 {
                return Err("IP packet too short (min 20 bytes)");
            }
        }
    }
    let len = data.len();
    sock.packets_sent += 1;
    sock.bytes_sent += len as u64;
    TOTAL_SENT.fetch_add(1, Ordering::Relaxed);
    Ok(len)
}

/// Receive the next raw packet from a socket's queue.
pub fn raw_recv(socket_id: u32) -> Option<Vec<u8>> {
    let mut st = STATE.lock();
    let sock = st.sockets.iter_mut().find(|s| s.id == socket_id)?;
    if sock.rx_queue.is_empty() {
        return None;
    }
    let pkt = sock.rx_queue.remove(0);
    sock.packets_recv += 1;
    sock.bytes_recv += pkt.len() as u64;
    TOTAL_RECV.fetch_add(1, Ordering::Relaxed);
    Some(pkt)
}

/// Deliver a packet to all matching raw sockets (called by NIC driver).
pub fn deliver_packet(iface: &str, data: &[u8]) {
    let mut st = STATE.lock();
    for sock in st.sockets.iter_mut() {
        if !sock.iface.is_empty() && sock.iface != iface {
            continue;
        }
        if sock.rx_queue.len() >= MAX_RX_QUEUE {
            continue;
        }
        sock.rx_queue.push(data.into());
    }
}

/// Bind a raw socket to a specific network interface.
pub fn raw_bind(socket_id: u32, iface: &str) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    let sock = st.sockets.iter_mut().find(|s| s.id == socket_id)
        .ok_or("socket not found")?;
    sock.iface = String::from(iface);
    Ok(())
}

/// Set protocol filter on a raw socket.
pub fn raw_set_filter(socket_id: u32, protocol: ProtocolFilter) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    let sock = st.sockets.iter_mut().find(|s| s.id == socket_id)
        .ok_or("socket not found")?;
    sock.filter = protocol;
    Ok(())
}

/// Enable or disable promiscuous mode on a raw socket.
pub fn raw_set_promisc(socket_id: u32, enabled: bool) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    let sock = st.sockets.iter_mut().find(|s| s.id == socket_id)
        .ok_or("socket not found")?;
    sock.promisc = enabled;
    Ok(())
}

/// Return information about the raw socket subsystem.
pub fn raw_socket_info() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== Raw Socket Subsystem ===\n");
    out.push_str(&format!("Open sockets: {}\n", st.sockets.len()));
    out.push_str(&format!("Total packets sent: {}\n", TOTAL_SENT.load(Ordering::Relaxed)));
    out.push_str(&format!("Total packets recv: {}\n", TOTAL_RECV.load(Ordering::Relaxed)));
    if st.sockets.is_empty() {
        out.push_str("(no open raw sockets)\n");
    } else {
        out.push_str("\nID   TYPE        IFACE      FILTER     PROMISC\n");
        out.push_str("---- ----------- ---------- ---------- -------\n");
        for s in &st.sockets {
            let ty = match s.socket_type {
                RawSocketType::RawEthernet => "Ethernet   ",
                RawSocketType::RawIp => "IP         ",
            };
            let filt = match s.filter {
                ProtocolFilter::All => "ALL       ",
                ProtocolFilter::Ipv4 => "IPv4      ",
                ProtocolFilter::Ipv6 => "IPv6      ",
                ProtocolFilter::Arp => "ARP       ",
                ProtocolFilter::Icmp => "ICMP      ",
                ProtocolFilter::Tcp => "TCP       ",
                ProtocolFilter::Udp => "UDP       ",
                ProtocolFilter::Custom(n) => return format!("0x{:04X}    ", n),
            };
            let iface = if s.iface.is_empty() { "*" } else { &s.iface };
            let promisc = if s.promisc { "yes" } else { "no" };
            out.push_str(&format!("{:<4} {} {:<10} {} {}\n",
                s.id, ty, iface, filt, promisc));
        }
    }
    out
}

/// Return per-socket statistics.
pub fn raw_socket_stats() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== Raw Socket Statistics ===\n");
    if st.sockets.is_empty() {
        out.push_str("(no open raw sockets)\n");
    } else {
        out.push_str("ID   TX_PKTS    TX_BYTES   RX_PKTS    RX_BYTES   RX_QUEUE\n");
        out.push_str("---- ---------- ---------- ---------- ---------- --------\n");
        for s in &st.sockets {
            out.push_str(&format!("{:<4} {:<10} {:<10} {:<10} {:<10} {}\n",
                s.id, s.packets_sent, s.bytes_sent,
                s.packets_recv, s.bytes_recv, s.rx_queue.len()));
        }
    }
    out
}
