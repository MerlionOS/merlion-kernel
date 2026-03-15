/// wget_real — fetch a web page using the real TCP stack.
/// Connects tcp_real + http + dns_client for end-to-end HTTP GET.
///
///   wget_real http://10.0.2.2:8080/   — fetch from QEMU host
///   wget_real http://example.com/      — fetch from internet (needs DHCP+DNS)

use alloc::string::String;
use alloc::vec::Vec;
use crate::{http, tcp_real, serial_println, println};

/// Fetch a URL using the real TCP stack.
pub fn fetch(url: &str) -> Result<String, &'static str> {
    // Parse URL
    let (host, port, path) = http::parse_url(url).ok_or("invalid URL")?;

    serial_println!("[wget_real] fetching {}:{}{}", host, port, path);
    println!("Resolving {}...", host);

    // Resolve hostname to IP
    let ip = resolve_host(&host)?;
    println!("Connecting to {}.{}.{}.{}:{}...", ip[0], ip[1], ip[2], ip[3], port);

    // TCP connect
    let sock_id = tcp_real::connect(crate::net::Ipv4Addr(ip), port)?;
    serial_println!("[wget_real] TCP connected, socket {}", sock_id);
    println!("Connected (socket {}).", sock_id);

    // Build and send HTTP request
    let request = http::build_request("GET", &host, &path);
    println!("Sending HTTP request ({} bytes)...", request.len());
    tcp_real::send(sock_id, &request)?;

    // Receive response (poll for data)
    println!("Waiting for response...");
    let mut response_data = Vec::new();
    let deadline = crate::timer::ticks() + crate::timer::PIT_FREQUENCY_HZ * 10; // 10 sec

    loop {
        // Poll incoming TCP segments
        tcp_real::poll_incoming();

        // Try to read data
        let data = tcp_real::recv(sock_id)?;
        if !data.is_empty() {
            response_data.extend_from_slice(&data);
            // Check if we got a complete HTTP response
            if response_data.windows(4).any(|w| w == b"\r\n\r\n") {
                // Got headers at least, check Content-Length
                if let Ok(resp) = http::parse_response(&response_data) {
                    let body_start = find_body_start(&response_data);
                    let content_length = get_content_length(&resp);
                    if let (Some(start), Some(cl)) = (body_start, content_length) {
                        if response_data.len() - start >= cl {
                            break; // Complete response
                        }
                    } else {
                        // No Content-Length or Connection: close — wait a bit more
                        let extra_wait = crate::timer::ticks() + 100;
                        while crate::timer::ticks() < extra_wait {
                            tcp_real::poll_incoming();
                            let more = tcp_real::recv(sock_id)?;
                            if !more.is_empty() {
                                response_data.extend_from_slice(&more);
                            }
                            x86_64::instructions::hlt();
                        }
                        break;
                    }
                }
            }
        }

        if crate::timer::ticks() > deadline {
            if response_data.is_empty() {
                let _ = tcp_real::close(sock_id);
                return Err("timeout: no response");
            }
            break; // Return whatever we got
        }

        x86_64::instructions::hlt();
    }

    // Close connection
    let _ = tcp_real::close(sock_id);

    // Parse and format response
    if response_data.is_empty() {
        return Err("empty response");
    }

    match http::parse_response(&response_data) {
        Ok(resp) => {
            serial_println!("[wget_real] got {} {}", resp.status_code, resp.status_text);
            Ok(http::format_response(&resp))
        }
        Err(_) => {
            // Return raw data if not valid HTTP
            Ok(String::from_utf8_lossy(&response_data).into_owned())
        }
    }
}

fn resolve_host(host: &str) -> Result<[u8; 4], &'static str> {
    // Try IP parsing
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 {
        if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
            parts[0].parse::<u8>(), parts[1].parse::<u8>(),
            parts[2].parse::<u8>(), parts[3].parse::<u8>(),
        ) {
            return Ok([a, b, c, d]);
        }
    }

    match host {
        "localhost" => Ok([127, 0, 0, 1]),
        "gateway" => Ok([10, 0, 2, 2]),
        _ => {
            // Try dns_client if available
            if let Some(ip) = crate::dhcp::resolve(host) {
                Ok(ip.0)
            } else {
                Err("cannot resolve hostname")
            }
        }
    }
}

fn find_body_start(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

fn get_content_length(resp: &http::HttpResponse) -> Option<usize> {
    for (key, val) in &resp.headers {
        if key.to_lowercase() == "content-length" {
            return val.trim().parse().ok();
        }
    }
    None
}
