/// wget — fetch a web page over HTTP/1.1 using the real TCP stack.
/// Integrates HTTP client with tcp_real for genuine TCP connections.
///
///   wget http://10.0.2.2:8080/       — fetch from host
///   wget http://example.com/          — fetch from internet (needs DNS)

use alloc::string::String;
use alloc::vec::Vec;
use crate::{http, serial_println, println, tcp_real, net};

/// Fetch a URL and display the result.
pub fn fetch(url: &str) -> Result<String, &'static str> {
    // Parse URL
    let (host, port, path) = http::parse_url(url).ok_or("invalid URL")?;

    serial_println!("[wget] fetching {}:{}{}", host, port, path);
    println!("Connecting to {}:{}...", host, port);

    // Build HTTP request
    let request = http::build_request("GET", &host, &path);

    // Resolve hostname to IP
    let ip = resolve_host(&host)?;
    println!("Resolved {} → {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);

    // Show the request that would be sent
    println!("Sending {} bytes...", request.len());
    serial_println!("[wget] request: {} bytes to {}:{}", request.len(), host, port);

    // Try to send via the network stack
    let response_data = try_network_fetch(ip, port, &request)?;

    // Parse HTTP response
    match http::parse_response(&response_data) {
        Ok(resp) => {
            let formatted = http::format_response(&resp);
            serial_println!("[wget] response: {} {}", resp.status_code, resp.status_text);
            Ok(formatted)
        }
        Err(e) => Err(e),
    }
}

/// Resolve a hostname to an IPv4 address.
fn resolve_host(host: &str) -> Result<[u8; 4], &'static str> {
    // Try direct IP parsing
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 {
        if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
            parts[0].parse::<u8>(), parts[1].parse::<u8>(),
            parts[2].parse::<u8>(), parts[3].parse::<u8>(),
        ) {
            return Ok([a, b, c, d]);
        }
    }

    // Known hosts
    match host {
        "localhost" => Ok([127, 0, 0, 1]),
        "gateway" => Ok([10, 0, 2, 2]),
        _ => {
            // Try DNS resolution via DHCP module
            if let Some(ip) = crate::dhcp::resolve(host) {
                Ok(ip.0)
            } else {
                Err("cannot resolve hostname")
            }
        }
    }
}

/// Fetch via the real TCP stack (tcp_real).
/// Performs a genuine 3-way handshake, sends the HTTP request, receives
/// the response, and tears down the connection.
fn try_network_fetch(ip: [u8; 4], port: u16, request: &[u8]) -> Result<Vec<u8>, &'static str> {
    let dst = net::Ipv4Addr(ip);

    // Open a real TCP connection
    let conn = tcp_real::connect(dst, port)?;
    serial_println!("[wget] TCP connected, conn={}", conn);

    // Wait for the connection to reach Established (SYN-ACK handshake).
    // Poll incoming segments while we wait.
    let deadline = crate::timer::ticks() + 500; // ~5 seconds at 100 Hz
    loop {
        tcp_real::poll_incoming();
        if let Some(tcp_real::TcpState::Established) = tcp_real::socket_state(conn) {
            break;
        }
        if crate::timer::ticks() > deadline {
            let _ = tcp_real::close(conn);
            return Err("TCP connect timeout");
        }
        x86_64::instructions::hlt();
    }
    println!("Connected.");

    // Send the HTTP request
    tcp_real::send(conn, request)?;
    serial_println!("[wget] sent {} bytes on conn {}", request.len(), conn);

    // Receive the response — keep polling until the connection closes
    // or we stop getting new data.
    let mut response = Vec::new();
    let mut idle_ticks: u64 = 0;
    let idle_limit: u64 = 200; // ~2 seconds of no new data
    loop {
        tcp_real::poll_incoming();
        match tcp_real::recv(conn) {
            Ok(data) if !data.is_empty() => {
                response.extend_from_slice(&data);
                idle_ticks = 0;
            }
            _ => {
                idle_ticks += 1;
                if idle_ticks > idle_limit {
                    break;
                }
                // Check if connection was closed by peer
                match tcp_real::socket_state(conn) {
                    Some(tcp_real::TcpState::CloseWait)
                    | Some(tcp_real::TcpState::Closed)
                    | Some(tcp_real::TcpState::TimeWait)
                    | None => {
                        // Drain any remaining data
                        if let Ok(data) = tcp_real::recv(conn) {
                            if !data.is_empty() {
                                response.extend_from_slice(&data);
                            }
                        }
                        break;
                    }
                    _ => {}
                }
                x86_64::instructions::hlt();
            }
        }
    }

    // Close our side
    let _ = tcp_real::close(conn);
    serial_println!("[wget] received {} bytes total", response.len());

    if response.is_empty() {
        return Err("empty response");
    }
    Ok(response)
}

/// Simple HTTP server response (for when MerlionOS acts as a server).
pub fn build_server_response(status: u16, body: &str) -> Vec<u8> {
    let status_text = match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let response = alloc::format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: text/html\r\n\
         Content-Length: {}\r\n\
         Server: MerlionOS/{}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        status, status_text, body.len(), crate::version::VERSION, body
    );
    response.into_bytes()
}
