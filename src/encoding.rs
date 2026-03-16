// SPDX-License-Identifier: MIT
//
// encoding.rs — Encoding/decoding utilities for MerlionOS
//
// Provides base64, hex, URL encoding/decoding, CRC-32, and FNV-1a hashing
// without any external crate dependencies.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Errors that can occur during decoding operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The input contained a byte that is not valid for the encoding.
    InvalidByte(u8),
    /// The input length is not valid for the encoding.
    InvalidLength,
    /// A percent-encoded sequence in a URL string was malformed.
    InvalidPercentEncoding,
    /// The decoded bytes are not valid UTF-8.
    InvalidUtf8,
}

/// A convenience `Result` type used throughout this module.
pub type Result<T> = core::result::Result<T, DecodeError>;

// ---------------------------------------------------------------------------
// Base64 (RFC 4648 standard alphabet, with `=` padding)
// ---------------------------------------------------------------------------

const B64_CHARS: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Returns the 6-bit value for a base64 ASCII character, or `Err` if invalid.
#[inline]
fn b64_decode_char(c: u8) -> Result<u8> {
    match c {
        b'A'..=b'Z' => Ok(c - b'A'),
        b'a'..=b'z' => Ok(c - b'a' + 26),
        b'0'..=b'9' => Ok(c - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(DecodeError::InvalidByte(c)),
    }
}

/// Encodes arbitrary bytes as a base64 string (RFC 4648, with padding).
///
/// # Examples
/// ```
/// assert_eq!(encoding::base64_encode(b"Hello"), "SGVsbG8=");
/// ```
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let (b0, b1, b2) = (
            chunk[0] as u32,
            if chunk.len() > 1 { chunk[1] as u32 } else { 0 },
            if chunk.len() > 2 { chunk[2] as u32 } else { 0 },
        );
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            out.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Decodes a base64 string back into raw bytes.
///
/// Returns `Err(DecodeError::InvalidLength)` when the input length (ignoring
/// padding) is not a multiple of 4, and `Err(DecodeError::InvalidByte(_))`
/// on encountering a character outside the base64 alphabet.
pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let input = s.as_bytes();
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if input.len() % 4 != 0 {
        return Err(DecodeError::InvalidLength);
    }

    let mut out = Vec::with_capacity(input.len() / 4 * 3);

    for chunk in input.chunks(4) {
        let pad = (chunk[2] == b'=') as usize + (chunk[3] == b'=') as usize;

        let v0 = b64_decode_char(chunk[0])? as u32;
        let v1 = b64_decode_char(chunk[1])? as u32;
        let v2 = if chunk[2] != b'=' { b64_decode_char(chunk[2])? as u32 } else { 0 };
        let v3 = if chunk[3] != b'=' { b64_decode_char(chunk[3])? as u32 } else { 0 };

        let triple = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;

        out.push((triple >> 16) as u8);
        if pad < 2 {
            out.push((triple >> 8) as u8);
        }
        if pad == 0 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Hexadecimal
// ---------------------------------------------------------------------------

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

/// Encodes bytes as a lowercase hexadecimal string.
///
/// Each input byte produces exactly two hex characters.
pub fn hex_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        out.push(HEX_LOWER[(b >> 4) as usize] as char);
        out.push(HEX_LOWER[(b & 0x0F) as usize] as char);
    }
    out
}

/// Decodes a hexadecimal string into bytes.
///
/// Accepts both uppercase and lowercase hex digits. Returns
/// `Err(DecodeError::InvalidLength)` if the string has an odd number of
/// characters.
pub fn hex_decode(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err(DecodeError::InvalidLength);
    }

    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

/// Converts a single ASCII hex digit to its 4-bit value.
#[inline]
fn hex_val(c: u8) -> Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(DecodeError::InvalidByte(c)),
    }
}

// ---------------------------------------------------------------------------
// Percent / URL encoding  (RFC 3986)
// ---------------------------------------------------------------------------

/// Returns `true` for characters that are *unreserved* per RFC 3986 and thus
/// do not need percent-encoding.
#[inline]
fn is_url_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~'
}

/// Percent-encodes a string following RFC 3986.
///
/// Unreserved characters (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`) are kept
/// as-is; every other byte is emitted as `%XX` in uppercase hex.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_url_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX_LOWER[(b >> 4) as usize].to_ascii_uppercase() as char);
            out.push(HEX_LOWER[(b & 0x0F) as usize].to_ascii_uppercase() as char);
        }
    }
    out
}

/// Decodes a percent-encoded string.
///
/// Returns `Err(DecodeError::InvalidPercentEncoding)` if a `%` is not
/// followed by exactly two hex digits, and `Err(DecodeError::InvalidUtf8)`
/// if the resulting byte sequence is not valid UTF-8.
pub fn url_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(DecodeError::InvalidPercentEncoding);
            }
            let hi = hex_val(bytes[i + 1]).map_err(|_| DecodeError::InvalidPercentEncoding)?;
            let lo = hex_val(bytes[i + 2]).map_err(|_| DecodeError::InvalidPercentEncoding)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).map_err(|_| DecodeError::InvalidUtf8)
}

// ---------------------------------------------------------------------------
// CRC-32 (ISO 3309 / ITU-T V.42, polynomial 0xEDB88320)
// ---------------------------------------------------------------------------

/// Pre-computed CRC-32 lookup table (reflected, polynomial `0xEDB88320`).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Computes the standard CRC-32 checksum (ISO 3309) of `data`.
///
/// This is the same algorithm used by zlib, gzip, PNG, and Ethernet FCS.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let index = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    crc ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// FNV-1a (64-bit)
// ---------------------------------------------------------------------------

/// FNV offset basis for the 64-bit variant.
const FNV1A_64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV prime for the 64-bit variant.
const FNV1A_64_PRIME: u64 = 0x0000_0100_0000_01B3;

/// Computes the 64-bit FNV-1a hash of `data`.
///
/// FNV-1a is a fast, non-cryptographic hash function with good distribution
/// properties, well-suited for hash tables and fingerprinting.
pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash = FNV1A_64_OFFSET;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV1A_64_PRIME);
    }
    hash
}
