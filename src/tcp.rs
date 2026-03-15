/// Minimal TCP stack.
/// Implements the TCP state machine, connection tracking, and
/// simulated 3-way handshake for educational purposes.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use spin::Mutex;
use crate::net::Ipv4Addr;

const MAX_CONNECTIONS: usize = 8;

/// TCP connection states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    Established,
    FinWait,
    TimeWait,
}

/// A TCP connection.
pub struct TcpConnection {
    pub local_port: u16,
    pub remote_ip: Ipv4Addr,
    pub remote_port: u16,
    pub state: TcpState,
    pub seq: u32,
    pub ack: u32,
    pub send_buf: Vec<u8>,
    pub recv_buf: Vec<u8>,
}

static CONNECTIONS: Mutex<Vec<TcpConnection>> = Mutex::new(Vec::new());

/// Open a TCP connection (simulated 3-way handshake).
pub fn connect(remote_ip: Ipv4Addr, remote_port: u16) -> Result<usize, &'static str> {
    let mut conns = CONNECTIONS.lock();
    if conns.len() >= MAX_CONNECTIONS {
        return Err("max connections reached");
    }

    let local_port = 49152 + conns.len() as u16; // ephemeral port

    // Simulate SYN → SYN-ACK → ACK
    crate::serial_println!("[tcp] SYN → {}:{}", remote_ip, remote_port);

    let is_reachable = remote_ip == Ipv4Addr::LOOPBACK
        || remote_ip == Ipv4Addr([10, 0, 2, 15])
        || remote_ip == Ipv4Addr([10, 0, 2, 2]);

    if !is_reachable {
        return Err("connection refused (host unreachable)");
    }

    crate::serial_println!("[tcp] SYN-ACK ← {}:{}", remote_ip, remote_port);
    crate::serial_println!("[tcp] ACK → established");

    let conn_id = conns.len();
    conns.push(TcpConnection {
        local_port,
        remote_ip,
        remote_port,
        state: TcpState::Established,
        seq: 1000,
        ack: 1,
        send_buf: Vec::new(),
        recv_buf: Vec::new(),
    });

    crate::klog_println!("[tcp] connected to {}:{} (conn {})", remote_ip, remote_port, conn_id);
    Ok(conn_id)
}

/// Send data on a connection.
pub fn send(conn_id: usize, data: &[u8]) -> Result<usize, &'static str> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(conn_id).ok_or("invalid connection")?;
    if conn.state != TcpState::Established {
        return Err("connection not established");
    }
    conn.seq += data.len() as u32;
    // Loopback: echo data back to recv buffer
    if conn.remote_ip == Ipv4Addr::LOOPBACK || conn.remote_ip == Ipv4Addr([10, 0, 2, 15]) {
        conn.recv_buf.extend_from_slice(data);
    }
    crate::serial_println!("[tcp] sent {} bytes on conn {}", data.len(), conn_id);
    Ok(data.len())
}

/// Receive data from a connection.
pub fn recv(conn_id: usize) -> Result<Vec<u8>, &'static str> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(conn_id).ok_or("invalid connection")?;
    if conn.state == TcpState::Closed {
        return Err("connection closed");
    }
    let data = core::mem::take(&mut conn.recv_buf);
    Ok(data)
}

/// Close a connection (simulated FIN handshake).
pub fn close(conn_id: usize) -> Result<(), &'static str> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns.get_mut(conn_id).ok_or("invalid connection")?;
    crate::serial_println!("[tcp] FIN → conn {} closing", conn_id);
    conn.state = TcpState::Closed;
    Ok(())
}

/// Listen on a port (stub).
pub fn listen(port: u16) -> Result<(), &'static str> {
    crate::serial_println!("[tcp] listening on port {}", port);
    crate::klog_println!("[tcp] listen :{}", port);
    Ok(())
}

/// List all connections.
pub fn list() -> Vec<(usize, String)> {
    let conns = CONNECTIONS.lock();
    conns.iter().enumerate().map(|(i, c)| {
        let state = match c.state {
            TcpState::Closed => "CLOSED",
            TcpState::Listen => "LISTEN",
            TcpState::SynSent => "SYN_SENT",
            TcpState::Established => "ESTABLISHED",
            TcpState::FinWait => "FIN_WAIT",
            TcpState::TimeWait => "TIME_WAIT",
        };
        (i, format!(":{} → {}:{} [{}]", c.local_port, c.remote_ip, c.remote_port, state))
    }).collect()
}
