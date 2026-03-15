/// Built-in HTTP/1.1 server for MerlionOS.
///
/// Provides a lightweight web server that runs inside the kernel, serving
/// system status pages, JSON API endpoints, and static files from the VFS.
/// Listens for TCP connections using [`crate::tcp_real`] primitives and
/// parses incoming HTTP requests to route them to registered handlers.
///
/// Built-in routes:
/// - `GET /`            — HTML dashboard with system information
/// - `GET /api/status`  — JSON object with uptime, memory, and task count
/// - `GET /api/tasks`   — JSON array of running tasks
/// - All other paths    — attempt to serve from VFS, or return 404

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::net;
use crate::tcp_real;
use crate::timer;
use crate::vfs;
use crate::version;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum size of a single HTTP request we will buffer (8 KiB).
const MAX_REQUEST_SIZE: usize = 8192;

/// Number of poll iterations when waiting for incoming data on a socket.
const RECV_POLL_LIMIT: usize = 500;

/// Default port for the built-in HTTP server.
const DEFAULT_PORT: u16 = 8080;

// ---------------------------------------------------------------------------
// HTTP method + parsed request
// ---------------------------------------------------------------------------

/// Supported HTTP methods.
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    /// GET request.
    Get,
    /// POST request.
    Post,
    /// HEAD request.
    Head,
    /// Any other method (stored as-is).
    Other(String),
}

/// A parsed incoming HTTP request.
#[derive(Debug, Clone)]
pub struct Request {
    /// HTTP method.
    pub method: Method,
    /// Request path, e.g. "/api/status".
    pub path: String,
    /// HTTP version string, e.g. "HTTP/1.1".
    pub version: String,
    /// Request headers as (name, value) pairs.
    pub headers: Vec<(String, String)>,
    /// Raw body bytes (empty for GET/HEAD).
    pub body: Vec<u8>,
}

/// An outgoing HTTP response (built programmatically).
#[derive(Debug, Clone)]
pub struct Response {
    /// Numeric status code.
    pub status: u16,
    /// Reason phrase.
    pub reason: String,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Route handler type
// ---------------------------------------------------------------------------

/// A route handler takes a parsed [`Request`] and returns a [`Response`].
pub type HandlerFn = fn(&Request) -> Response;

/// A single route entry: (method, path_prefix, handler).
pub struct Route {
    /// HTTP method to match (None = match any).
    pub method: Option<Method>,
    /// Path prefix to match (exact match).
    pub path: String,
    /// Handler function.
    pub handler: HandlerFn,
}

// ---------------------------------------------------------------------------
// HttpServer
// ---------------------------------------------------------------------------

/// The built-in MerlionOS HTTP server.
///
/// Holds a table of routes and listens on a TCP port. Incoming connections
/// are handled sequentially (one at a time) — appropriate for a kernel-mode
/// diagnostic server.
pub struct HttpServer {
    /// Registered routes checked in order; first match wins.
    pub routes: Vec<Route>,
    /// TCP port to listen on.
    pub port: u16,
}

impl HttpServer {
    /// Create a new HTTP server with the built-in routes pre-registered.
    pub fn new() -> Self {
        let mut server = Self {
            routes: Vec::new(),
            port: DEFAULT_PORT,
        };
        server.routes.push(Route {
            method: Some(Method::Get),
            path: "/".to_owned(),
            handler: handle_index,
        });
        server.routes.push(Route {
            method: Some(Method::Get),
            path: "/api/status".to_owned(),
            handler: handle_api_status,
        });
        server.routes.push(Route {
            method: Some(Method::Get),
            path: "/api/tasks".to_owned(),
            handler: handle_api_tasks,
        });
        server
    }

    /// Start listening for HTTP connections on `port`.
    ///
    /// This function enters an infinite loop, accepting one TCP connection
    /// at a time via [`crate::tcp_real`]. Each connection is read,
    /// parsed, routed to a handler, and the response is sent back before
    /// the socket is closed.
    ///
    /// The loop runs until the kernel task is killed or the system shuts
    /// down; it yields between accept attempts to avoid starving other
    /// kernel tasks.
    pub fn start(&self, port: u16) {
        let local_ip = net::NET.lock().ip;
        crate::serial_println!("[httpd] starting on {}:{}", local_ip, port);
        crate::println!("[httpd] listening on http://{}:{}/", local_ip, port);

        loop {
            // Poll for an incoming SYN on our port.
            if let Some((request_data, sock_id)) = self.accept_connection(port) {
                // Parse the HTTP request.
                match parse_request(&request_data) {
                    Ok(req) => {
                        crate::serial_println!(
                            "[httpd] {} {} from sock {}",
                            method_str(&req.method),
                            req.path,
                            sock_id
                        );
                        let resp = self.route(&req);
                        let raw = serialize_response(&resp);
                        let _ = tcp_real::send(sock_id, &raw);
                    }
                    Err(e) => {
                        crate::serial_println!("[httpd] bad request: {}", e);
                        let resp = response_text(400, "Bad Request", e);
                        let raw = serialize_response(&resp);
                        let _ = tcp_real::send(sock_id, &raw);
                    }
                }
                // Brief wait for the data to flush, then close.
                busy_wait_ticks(2);
                let _ = tcp_real::close(sock_id);
            }

            // Yield to other tasks between accept attempts.
            crate::task::yield_now();
        }
    }

    /// Wait for an incoming TCP connection on `port`.
    ///
    /// Polls the network stack for a SYN destined for our port, completes
    /// the 3-way handshake (server side), then reads the full request.
    /// Returns the raw request bytes and socket index on success.
    fn accept_connection(&self, port: u16) -> Option<(Vec<u8>, usize)> {
        use crate::netstack;
        use crate::net::ETH_TYPE_IP;

        let frame = netstack::poll_rx()?;
        if frame.ethertype != ETH_TYPE_IP {
            return None;
        }

        let ip = &frame.payload;
        if ip.len() < 20 || ip[9] != 6 {
            return None;
        }

        let ihl = ((ip[0] & 0x0F) as usize) * 4;
        if ip.len() < ihl + 20 {
            return None;
        }
        let tcp_data = &ip[ihl..];
        let hdr = tcp_real::parse_tcp_header(tcp_data)?;
        let flags = tcp_real::header_flags(&hdr);

        // Only accept SYN (not SYN+ACK) destined for our port.
        if hdr.dst_port != port || flags != tcp_real::TCP_SYN {
            return None;
        }

        let mut peer_ip = [0u8; 4];
        peer_ip.copy_from_slice(&ip[12..16]);
        let local_ip = net::NET.lock().ip;

        // Complete the handshake: send SYN-ACK.
        let isn = (timer::ticks().wrapping_mul(2654435761)) as u32;
        let peer_seq = hdr.seq;
        let ack_num = peer_seq.wrapping_add(1);

        let syn_ack = tcp_real::build_tcp_packet(
            local_ip.0,
            peer_ip,
            port,
            hdr.src_port,
            isn,
            ack_num,
            tcp_real::TCP_SYN | tcp_real::TCP_ACK,
            &[],
        );
        netstack::send_ipv4(peer_ip, 6, &syn_ack);

        // Wait for the final ACK of our SYN-ACK.
        let mut established = false;
        for _ in 0..200 {
            if let Some(frame2) = netstack::poll_rx() {
                if frame2.ethertype != ETH_TYPE_IP {
                    continue;
                }
                let ip2 = &frame2.payload;
                if ip2.len() < 20 || ip2[9] != 6 {
                    continue;
                }
                let ihl2 = ((ip2[0] & 0x0F) as usize) * 4;
                if ip2.len() < ihl2 + 20 {
                    continue;
                }
                let tcp2 = &ip2[ihl2..];
                let hdr2 = tcp_real::parse_tcp_header(tcp2);
                if let Some(h2) = hdr2 {
                    let f2 = tcp_real::header_flags(&h2);
                    if h2.dst_port == port && f2 & tcp_real::TCP_ACK != 0 && f2 & tcp_real::TCP_SYN == 0 {
                        established = true;
                        break;
                    }
                }
            }
            busy_wait_ticks(1);
        }

        if !established {
            return None;
        }

        // Connection established — create a pseudo-socket index for tracking.
        // Read the HTTP request data from subsequent segments.
        let our_seq = isn.wrapping_add(1);
        let mut recv_buf: Vec<u8> = Vec::new();
        let mut current_ack = ack_num;

        for _ in 0..RECV_POLL_LIMIT {
            if let Some(frame3) = netstack::poll_rx() {
                if frame3.ethertype != ETH_TYPE_IP {
                    continue;
                }
                let ip3 = &frame3.payload;
                if ip3.len() < 20 || ip3[9] != 6 {
                    continue;
                }
                let ihl3 = ((ip3[0] & 0x0F) as usize) * 4;
                if ip3.len() < ihl3 + 20 {
                    continue;
                }
                let tcp3 = &ip3[ihl3..];
                let payload = tcp_real::segment_payload(tcp3);
                if !payload.is_empty() {
                    recv_buf.extend_from_slice(payload);
                    current_ack = current_ack.wrapping_add(payload.len() as u32);

                    // ACK the received data.
                    let ack_pkt = tcp_real::build_tcp_packet(
                        local_ip.0,
                        peer_ip,
                        port,
                        hdr.src_port,
                        our_seq,
                        current_ack,
                        tcp_real::TCP_ACK,
                        &[],
                    );
                    netstack::send_ipv4(peer_ip, 6, &ack_pkt);

                    // Check if we have a complete HTTP request (ends with \r\n\r\n).
                    if recv_buf.len() >= 4 && has_header_end(&recv_buf) {
                        break;
                    }
                }
                if recv_buf.len() >= MAX_REQUEST_SIZE {
                    break;
                }
            }
            busy_wait_ticks(1);
        }

        if recv_buf.is_empty() {
            return None;
        }

        // Register a socket in tcp_real for sending the response.
        let sock_id = register_server_socket(local_ip, port, peer_ip, hdr.src_port, our_seq, current_ack);

        Some((recv_buf, sock_id))
    }

    /// Route a parsed request to the first matching handler.
    ///
    /// If no route matches, attempts to serve a static file from the VFS.
    /// Returns a 404 response if nothing is found.
    fn route(&self, req: &Request) -> Response {
        for route in &self.routes {
            if let Some(ref m) = route.method {
                if *m != req.method {
                    continue;
                }
            }
            if req.path == route.path {
                return (route.handler)(req);
            }
        }
        // Fall through: try VFS static file serving.
        serve_vfs_file(&req.path)
    }
}

// ---------------------------------------------------------------------------
// Request parsing
// ---------------------------------------------------------------------------

/// Parse a raw HTTP request from bytes.
///
/// Splits the request into method, path, version, headers, and body.
/// Only the header section needs to be valid UTF-8.
fn parse_request(data: &[u8]) -> Result<Request, &'static str> {
    let boundary = find_header_boundary(data).ok_or("incomplete request headers")?;
    let header_bytes = &data[..boundary];
    let body = if boundary + 4 <= data.len() {
        data[boundary + 4..].to_vec()
    } else {
        Vec::new()
    };

    let header_str = core::str::from_utf8(header_bytes)
        .map_err(|_| "request headers not valid UTF-8")?;

    let mut lines = header_str.split("\r\n");
    let request_line = lines.next().ok_or("empty request")?;

    // Parse "GET /path HTTP/1.1"
    let mut parts = request_line.splitn(3, ' ');
    let method_str_val = parts.next().ok_or("missing method")?;
    let path = parts.next().ok_or("missing path")?.to_owned();
    let version = parts.next().unwrap_or("HTTP/1.1").to_owned();

    let method = match method_str_val {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "HEAD" => Method::Head,
        other => Method::Other(other.to_owned()),
    };

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some(colon) = line.find(':') {
            let name = line[..colon].trim().to_owned();
            let value = line[colon + 1..].trim().to_owned();
            headers.push((name, value));
        }
    }

    Ok(Request { method, path, version, headers, body })
}

/// Locate the `\r\n\r\n` header boundary in raw bytes.
fn find_header_boundary(data: &[u8]) -> Option<usize> {
    let marker = b"\r\n\r\n";
    if data.len() < marker.len() {
        return None;
    }
    for i in 0..=data.len() - marker.len() {
        if &data[i..i + marker.len()] == marker {
            return Some(i);
        }
    }
    None
}

/// Check if the buffer contains `\r\n\r\n` (complete headers).
fn has_header_end(data: &[u8]) -> bool {
    find_header_boundary(data).is_some()
}

// ---------------------------------------------------------------------------
// Response serialization
// ---------------------------------------------------------------------------

/// Serialize an HTTP response to raw bytes ready for transmission.
fn serialize_response(resp: &Response) -> Vec<u8> {
    let mut buf = format!("HTTP/1.1 {} {}\r\n", resp.status, resp.reason);
    for (name, value) in &resp.headers {
        buf.push_str(name);
        buf.push_str(": ");
        buf.push_str(value);
        buf.push_str("\r\n");
    }
    // Always include Content-Length and Connection: close.
    buf.push_str(&format!("Content-Length: {}\r\n", resp.body.len()));
    buf.push_str("Connection: close\r\n");
    buf.push_str("Server: MerlionOS-httpd\r\n");
    buf.push_str("\r\n");

    let mut raw = buf.into_bytes();
    raw.extend_from_slice(&resp.body);
    raw
}

// ---------------------------------------------------------------------------
// Built-in route handlers
// ---------------------------------------------------------------------------

/// `GET /` — returns an HTML page with system information and MerlionOS branding.
fn handle_index(_req: &Request) -> Response {
    let (h, m, s) = timer::uptime_hms();
    let heap = crate::allocator::stats();
    let tasks = crate::task::list();
    let net_state = net::NET.lock();
    let ip = format!("{}", net_state.ip);
    let tx = net_state.tx_packets;
    let rx = net_state.rx_packets;
    drop(net_state);

    let html = format!(
r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name} Dashboard</title>
<style>
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
         background: #0a0e17; color: #c9d1d9; margin: 0; padding: 0; }}
  header {{ background: linear-gradient(135deg, #1a1f36, #0d47a1); padding: 24px 32px;
            border-bottom: 2px solid #30a3e6; }}
  header h1 {{ margin: 0; color: #58a6ff; font-size: 1.8em; }}
  header p {{ margin: 4px 0 0; color: #8b949e; font-style: italic; }}
  .container {{ max-width: 800px; margin: 24px auto; padding: 0 16px; }}
  .card {{ background: #161b22; border: 1px solid #30363d; border-radius: 8px;
           padding: 16px 20px; margin-bottom: 16px; }}
  .card h2 {{ color: #58a6ff; margin: 0 0 12px; font-size: 1.1em;
              border-bottom: 1px solid #21262d; padding-bottom: 8px; }}
  .stat {{ display: flex; justify-content: space-between; padding: 4px 0; }}
  .stat .label {{ color: #8b949e; }}
  .stat .value {{ color: #f0f6fc; font-family: monospace; }}
  .task-list {{ list-style: none; padding: 0; margin: 0; }}
  .task-list li {{ padding: 4px 0; font-family: monospace; color: #c9d1d9; }}
  footer {{ text-align: center; padding: 16px; color: #484f58; font-size: 0.85em; }}
</style>
</head>
<body>
<header>
  <h1>{name}</h1>
  <p>{slogan}</p>
</header>
<div class="container">
  <div class="card">
    <h2>System Status</h2>
    <div class="stat"><span class="label">Version</span><span class="value">{ver}</span></div>
    <div class="stat"><span class="label">Uptime</span><span class="value">{h:02}:{m:02}:{s:02}</span></div>
    <div class="stat"><span class="label">Tasks</span><span class="value">{ntasks} running</span></div>
    <div class="stat"><span class="label">Heap</span><span class="value">{heap_used} / {heap_total} bytes ({heap_pct}%)</span></div>
    <div class="stat"><span class="label">IP Address</span><span class="value">{ip}</span></div>
    <div class="stat"><span class="label">Packets TX/RX</span><span class="value">{tx} / {rx}</span></div>
  </div>
  <div class="card">
    <h2>Running Tasks</h2>
    <ul class="task-list">{task_html}</ul>
  </div>
  <div class="card">
    <h2>API Endpoints</h2>
    <div class="stat"><span class="label">GET /api/status</span><span class="value">System status JSON</span></div>
    <div class="stat"><span class="label">GET /api/tasks</span><span class="value">Task list JSON</span></div>
  </div>
</div>
<footer>{name} {version} &mdash; {slogan}</footer>
</body>
</html>"#,
        name = version::NAME,
        slogan = version::SLOGAN,
        ver = version::full(),
        h = h, m = m, s = s,
        ntasks = tasks.len(),
        heap_used = heap.used,
        heap_total = heap.total,
        heap_pct = if heap.total > 0 { heap.used * 100 / heap.total } else { 0 },
        ip = ip,
        tx = tx,
        rx = rx,
        version = version::VERSION,
        task_html = format_task_html(&tasks),
    );

    response_html(200, "OK", &html)
}

/// `GET /api/status` — returns JSON with uptime, memory usage, and task count.
fn handle_api_status(_req: &Request) -> Response {
    let uptime = timer::uptime_secs();
    let (h, m, s) = timer::uptime_hms();
    let heap = crate::allocator::stats();
    let tasks = crate::task::list();
    let ticks = timer::ticks();

    let json = format!(
        concat!(
            "{{",
            "\"version\":\"{}\",",
            "\"uptime_secs\":{},",
            "\"uptime_formatted\":\"{:02}:{:02}:{:02}\",",
            "\"ticks\":{},",
            "\"heap_used\":{},",
            "\"heap_total\":{},",
            "\"heap_free\":{},",
            "\"task_count\":{}",
            "}}"
        ),
        version::full(),
        uptime,
        h, m, s,
        ticks,
        heap.used,
        heap.total,
        heap.free,
        tasks.len(),
    );

    response_json(200, "OK", &json)
}

/// `GET /api/tasks` — returns a JSON array of running tasks.
fn handle_api_tasks(_req: &Request) -> Response {
    let tasks = crate::task::list();
    let mut json = String::from("[");

    for (i, t) in tasks.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let state_str = match t.state {
            crate::task::TaskState::Ready => "ready",
            crate::task::TaskState::Running => "running",
            crate::task::TaskState::Finished => "finished",
        };
        json.push_str(&format!(
            "{{\"pid\":{},\"name\":\"{}\",\"state\":\"{}\"}}",
            t.pid, t.name, state_str
        ));
    }

    json.push(']');
    response_json(200, "OK", &json)
}

// ---------------------------------------------------------------------------
// VFS static file serving
// ---------------------------------------------------------------------------

/// Attempt to serve a file from the VFS at the given path.
///
/// Maps the URL path directly to a VFS path. Returns a 404 response if
/// the file is not found. Guesses Content-Type from the file extension.
fn serve_vfs_file(path: &str) -> Response {
    match vfs::cat(path) {
        Ok(content) => {
            let content_type = guess_content_type(path);
            let mut resp = Response {
                status: 200,
                reason: "OK".to_owned(),
                headers: Vec::new(),
                body: content.into_bytes(),
            };
            resp.headers.push(("Content-Type".to_owned(), content_type.to_owned()));
            resp
        }
        Err(_) => response_text(404, "Not Found", "404 Not Found"),
    }
}

/// Guess a MIME content type from the file extension.
fn guess_content_type(path: &str) -> &'static str {
    if path.ends_with(".html") || path.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".txt") {
        "text/plain; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    }
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

/// Build a plain-text HTTP response.
fn response_text(status: u16, reason: &str, body: &str) -> Response {
    Response {
        status,
        reason: reason.to_owned(),
        headers: vec![("Content-Type".to_owned(), "text/plain; charset=utf-8".to_owned())],
        body: body.as_bytes().to_vec(),
    }
}

/// Build an HTML HTTP response.
fn response_html(status: u16, reason: &str, html: &str) -> Response {
    Response {
        status,
        reason: reason.to_owned(),
        headers: vec![("Content-Type".to_owned(), "text/html; charset=utf-8".to_owned())],
        body: html.as_bytes().to_vec(),
    }
}

/// Build a JSON HTTP response.
fn response_json(status: u16, reason: &str, json: &str) -> Response {
    Response {
        status,
        reason: reason.to_owned(),
        headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
        body: json.as_bytes().to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Helper: format task list as HTML <li> elements
// ---------------------------------------------------------------------------

/// Render the task list as HTML list items for the dashboard.
fn format_task_html(tasks: &[crate::task::TaskInfo]) -> String {
    let mut html = String::new();
    for t in tasks {
        let state = match t.state {
            crate::task::TaskState::Ready => "ready",
            crate::task::TaskState::Running => "running",
            crate::task::TaskState::Finished => "finished",
        };
        html.push_str(&format!(
            "<li>[pid {}] {} ({})</li>",
            t.pid, t.name, state
        ));
    }
    html
}

/// Convert a [`Method`] to its string representation.
fn method_str(m: &Method) -> &str {
    match m {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Head => "HEAD",
        Method::Other(s) => s.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Socket registration helper
// ---------------------------------------------------------------------------

/// Register a server-side socket in the tcp_real socket table so we can
/// use [`tcp_real::send`] and [`tcp_real::close`] for the response.
///
/// This is a workaround because tcp_real does not yet have a proper
/// listen/accept API — we perform the handshake manually in
/// [`HttpServer::accept_connection`] and then register the resulting
/// connection state.
fn register_server_socket(
    local_ip: crate::net::Ipv4Addr,
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    seq_num: u32,
    ack_num: u32,
) -> usize {
    // We push directly into the tcp_real SOCKETS table.
    // This requires the SOCKETS static to be accessible — we re-use the
    // public connect/send/close API after manually inserting.
    use crate::tcp_real::{TcpSocket, TcpState};

    // Access the global socket table through a small shim.
    // Since SOCKETS is private in tcp_real, we use an alternate approach:
    // open a "connection" that is already in Established state by calling
    // the internal registration.
    crate::tcp_real::register_established(
        local_ip,
        local_port,
        crate::net::Ipv4Addr(remote_ip),
        remote_port,
        seq_num,
        ack_num,
    )
}

// ---------------------------------------------------------------------------
// Shell command entry point
// ---------------------------------------------------------------------------

/// Shell command: `serve [port]`
///
/// Starts the built-in HTTP server. If no port is given, defaults to 8080.
/// The server runs in the foreground and never returns (kill the task to
/// stop it).
pub fn cmd_serve(args: &str) {
    let port = if args.is_empty() {
        DEFAULT_PORT
    } else {
        parse_u16_simple(args.trim()).unwrap_or(DEFAULT_PORT)
    };

    crate::println!("[httpd] MerlionOS built-in HTTP server");
    crate::println!("[httpd] starting on port {} ...", port);

    let server = HttpServer::new();
    server.start(port);
}

/// Parse a decimal string to u16 without std.
fn parse_u16_simple(s: &str) -> Option<u16> {
    if s.is_empty() {
        return None;
    }
    let mut n: u16 = 0;
    for b in s.bytes() {
        let digit = b.wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(digit as u16)?;
    }
    Some(n)
}

/// Busy-wait for `n` PIT ticks (~10 ms each at 100 Hz).
fn busy_wait_ticks(n: u64) {
    let target = timer::ticks() + n;
    while timer::ticks() < target {
        x86_64::instructions::hlt();
    }
}
