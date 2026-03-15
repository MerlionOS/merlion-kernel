/// wget — fetch a web page over HTTP/1.1.
/// Integrates HTTP client with the TCP/network stack.
///
///   wget http://10.0.2.2:8080/       — fetch from host
///   wget http://example.com/          — fetch from internet (needs DNS+TCP)
///
/// Current implementation builds the HTTP request and shows what
/// would be sent. Full end-to-end requires tcp_real.rs to be
/// connected to netstack.rs.

use alloc::string::String;
use alloc::vec::Vec;
use crate::{http, serial_println, println, print};

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

/// Attempt to fetch via the real network stack.
/// Returns the raw HTTP response bytes.
fn try_network_fetch(ip: [u8; 4], port: u16, request: &[u8]) -> Result<Vec<u8>, &'static str> {
    // For now, construct a simulated response for localhost/gateway
    // Real implementation requires tcp_real.rs connection
    if ip == [127, 0, 0, 1] || ip == [10, 0, 2, 15] {
        // Loopback: generate a local response
        let body = alloc::format!(
            "<html><body><h1>MerlionOS</h1>\
             <p>Born for AI. Built by AI.</p>\
             <p>Served from localhost</p></body></html>"
        );
        let response = alloc::format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html\r\n\
             Content-Length: {}\r\n\
             Server: MerlionOS/7.0\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            body.len(), body
        );
        return Ok(response.into_bytes());
    }

    // For external hosts, send via netstack (UDP for now as TCP isn't connected)
    // This is a placeholder — real TCP connection needed
    crate::netstack::send_udp(ip, 12345, port, request);
    println!("Request sent via UDP (TCP connection pending).");

    // Generate a status response
    let response = alloc::format!(
        "HTTP/1.1 000 Pending\r\n\
         X-MerlionOS: TCP stack not yet connected to netstack\r\n\
         X-Request-Sent-To: {}.{}.{}.{}:{}\r\n\
         \r\n\
         (Awaiting tcp_real.rs integration for full HTTP fetch)",
        ip[0], ip[1], ip[2], ip[3], port
    );
    Ok(response.into_bytes())
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
