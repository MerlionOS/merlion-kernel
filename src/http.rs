/// Minimal HTTP/1.1 client for MerlionOS.
///
/// Provides request building, response parsing, and URL handling.
/// Uses `#![no_std]`-compatible types from `alloc`. Actual TCP
/// transport is handled elsewhere; this module deals only with
/// the HTTP wire format.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An outgoing HTTP request (structured form).
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// HTTP method, e.g. "GET", "POST".
    pub method: String,
    /// Target host (used for the Host header).
    pub host: String,
    /// Request path, e.g. "/index.html".
    pub path: String,
    /// Additional headers as (name, value) pairs.
    pub headers: Vec<(String, String)>,
}

/// A parsed HTTP response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// Numeric status code, e.g. 200.
    pub status_code: u16,
    /// Reason phrase, e.g. "OK".
    pub status_text: String,
    /// Response headers as (name, value) pairs.
    pub headers: Vec<(String, String)>,
    /// Raw response body bytes.
    pub body: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// Build a raw HTTP/1.1 request as bytes.
///
/// The request includes a `Host` header, `Connection: close`, and
/// a `User-Agent: MerlionOS/5.0` header. No body is attached.
///
/// # Example (conceptual)
/// ```ignore
/// let raw = build_request("GET", "example.com", "/");
/// // raw == b"GET / HTTP/1.1\r\nHost: example.com\r\n..."
/// ```
pub fn build_request(method: &str, host: &str, path: &str) -> Vec<u8> {
    let req = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Connection: close\r\n\
         User-Agent: MerlionOS/5.0\r\n\
         \r\n"
    );
    req.into_bytes()
}

/// Build raw request bytes from an [`HttpRequest`] struct.
///
/// All headers stored in the struct are emitted after the default
/// `Host`, `Connection`, and `User-Agent` headers.
pub fn build_request_from(req: &HttpRequest) -> Vec<u8> {
    let mut buf = format!(
        "{} {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Connection: close\r\n\
         User-Agent: MerlionOS/5.0\r\n",
        req.method, req.path, req.host,
    );
    for (name, value) in &req.headers {
        buf.push_str(name);
        buf.push_str(": ");
        buf.push_str(value);
        buf.push_str("\r\n");
    }
    buf.push_str("\r\n");
    buf.into_bytes()
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Parse a complete HTTP/1.1 response from raw bytes.
///
/// Splits on the first `\r\n\r\n` boundary to separate headers from
/// body. Returns an error if the status line or header section is
/// malformed.
pub fn parse_response(data: &[u8]) -> Result<HttpResponse, &'static str> {
    // Locate header/body boundary.
    let boundary = find_header_end(data).ok_or("missing header/body boundary")?;
    let header_bytes = &data[..boundary];
    let body = data[boundary + 4..].to_vec(); // skip \r\n\r\n

    // Convert header section to UTF-8.
    let header_str = core::str::from_utf8(header_bytes)
        .map_err(|_| "headers are not valid UTF-8")?;

    let mut lines = header_str.split("\r\n");

    // --- Status line: "HTTP/1.1 200 OK" ---
    let status_line = lines.next().ok_or("empty response")?;
    let (status_code, status_text) = parse_status_line(status_line)?;

    // --- Headers ---
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let colon = line.find(':').ok_or("malformed header line")?;
        let name = line[..colon].trim().to_owned();
        let value = line[colon + 1..].trim().to_owned();
        headers.push((name, value));
    }

    Ok(HttpResponse {
        status_code,
        status_text,
        headers,
        body,
    })
}

/// Parse the status line into (code, reason).
fn parse_status_line(line: &str) -> Result<(u16, String), &'static str> {
    // "HTTP/1.x <code> <reason>"
    let rest = line.strip_prefix("HTTP/1.")
        .ok_or("status line missing HTTP/1.x prefix")?;
    // Skip version digit + space.
    let after_ver = rest.get(1..).ok_or("truncated status line")?;
    let after_ver = after_ver.trim_start();

    let space = after_ver.find(' ').ok_or("no reason phrase")?;
    let code_str = &after_ver[..space];
    let code = parse_u16(code_str).ok_or("invalid status code")?;
    let reason = after_ver[space + 1..].to_owned();

    Ok((code, reason))
}

/// Locate the `\r\n\r\n` boundary. Returns the byte offset of the
/// first `\r` in the sequence.
fn find_header_end(data: &[u8]) -> Option<usize> {
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

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

/// Parse an HTTP URL into `(host, port, path)`.
///
/// Only the `http://` scheme is accepted. If the port is omitted it
/// defaults to 80. The path defaults to `"/"` when absent.
///
/// # Examples (conceptual)
/// ```ignore
/// let (h, p, path) = parse_url("http://example.com:8080/api").unwrap();
/// assert_eq!(h, "example.com");
/// assert_eq!(p, 8080);
/// assert_eq!(path, "/api");
/// ```
pub fn parse_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;

    // Split host+port from path at the first '/'.
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_owned()),
        None => (rest, "/".to_owned()),
    };

    // Split host from port at ':'.
    let (host, port) = match host_port.rfind(':') {
        Some(i) => {
            let p = parse_u16(&host_port[i + 1..])?;
            (host_port[..i].to_owned(), p)
        }
        None => (host_port.to_owned(), 80),
    };

    if host.is_empty() {
        return None;
    }

    Some((host, port, path))
}

// ---------------------------------------------------------------------------
// High-level helpers
// ---------------------------------------------------------------------------

/// Perform a GET request (build phase only).
///
/// Parses the URL, constructs a full HTTP/1.1 GET request, and
/// returns the raw bytes ready to send over a TCP socket. Actual
/// network transmission is handled by the TCP stack and is outside
/// the scope of this module.
pub fn get(url: &str) -> Result<HttpRequest, &'static str> {
    let (host, _port, path) = parse_url(url).ok_or("invalid URL")?;

    Ok(HttpRequest {
        method: "GET".to_owned(),
        host,
        path,
        headers: Vec::new(),
    })
}

/// Format an [`HttpResponse`] as a human-readable string.
///
/// Includes the status line, all headers, and the body decoded as
/// UTF-8 (lossy). Useful for the MerlionOS shell `curl` command.
pub fn format_response(resp: &HttpResponse) -> String {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\n",
        resp.status_code, resp.status_text
    );
    for (name, value) in &resp.headers {
        out.push_str(&format!("{}: {}\r\n", name, value));
    }
    out.push_str("\r\n");

    // Best-effort UTF-8 body rendering.
    match core::str::from_utf8(&resp.body) {
        Ok(s) => out.push_str(s),
        Err(_) => out.push_str(&format!("[binary body, {} bytes]", resp.body.len())),
    }
    out
}

// ---------------------------------------------------------------------------
// Tiny no_std helpers
// ---------------------------------------------------------------------------

/// Parse a decimal string into `u16` without pulling in `str::parse`.
fn parse_u16(s: &str) -> Option<u16> {
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
