// SPDX-License-Identifier: MIT
//
// compress.rs — LZ77-based data compression for MerlionOS
//
// Provides a sliding-window (4096-byte) compressor and decompressor with a
// compact binary token format.  No external crate dependencies.

extern crate alloc;

use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Size of the sliding window used by the compressor (4 KiB).
const WINDOW_SIZE: usize = 4096;

/// Minimum match length worth encoding as a back-reference.  Matches shorter
/// than this are cheaper to emit as literals.
const MIN_MATCH_LEN: usize = 3;

/// Maximum match length we can encode in our binary format (stored as a single
/// byte minus `MIN_MATCH_LEN`, giving a range of 3..=258).
const MAX_MATCH_LEN: usize = 258;

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A single LZ77 token — either a literal byte or a (distance, length)
/// back-reference into the sliding window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// An uncompressed literal byte.
    Literal(u8),
    /// A back-reference: copy `length` bytes starting `offset` bytes back in
    /// the output stream.  `offset` is in `1..=WINDOW_SIZE` and `length` is
    /// in `MIN_MATCH_LEN..=MAX_MATCH_LEN`.
    Match { offset: u16, length: u16 },
}

// ---------------------------------------------------------------------------
// LZ77 tokenisation (compress side)
// ---------------------------------------------------------------------------

/// Scan the sliding window for the longest match starting at `data[pos]`.
///
/// Returns `(best_offset, best_length)` where `best_offset` is the distance
/// back from `pos` (1-based) and `best_length` is the number of matching
/// bytes.  If no match of at least `MIN_MATCH_LEN` is found, returns `(0, 0)`.
fn find_longest_match(data: &[u8], pos: usize) -> (u16, u16) {
    let window_start = if pos >= WINDOW_SIZE { pos - WINDOW_SIZE } else { 0 };
    let remaining = data.len() - pos;
    let max_len = if remaining < MAX_MATCH_LEN { remaining } else { MAX_MATCH_LEN };

    let mut best_offset: u16 = 0;
    let mut best_length: u16 = 0;

    let mut candidate = window_start;
    while candidate < pos {
        let mut len: usize = 0;
        while len < max_len && data[candidate + len] == data[pos + len] {
            len += 1;
        }
        if len >= MIN_MATCH_LEN && len as u16 > best_length {
            best_offset = (pos - candidate) as u16;
            best_length = len as u16;
            if len == max_len {
                break; // can't do better
            }
        }
        candidate += 1;
    }

    (best_offset, best_length)
}

/// Tokenise `data` using LZ77 with a 4096-byte sliding window.
///
/// Returns a vector of [`Token`]s that can later be serialised with
/// [`encode_tokens`].
pub fn tokenise(data: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos: usize = 0;

    while pos < data.len() {
        let (offset, length) = find_longest_match(data, pos);
        if length >= MIN_MATCH_LEN as u16 {
            tokens.push(Token::Match { offset, length });
            pos += length as usize;
        } else {
            tokens.push(Token::Literal(data[pos]));
            pos += 1;
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Binary encoding / decoding of token streams
// ---------------------------------------------------------------------------

/// Encode a token stream into a compact binary format.
///
/// # Format
///
/// Tokens are grouped into blocks of up to 8.  Each block starts with a
/// **flags byte** where bit *i* indicates whether the *i*-th token is a
/// `Match` (1) or a `Literal` (0).  Literals are stored as a single byte;
/// matches are stored as 3 bytes: offset high (4 bits) | length-MIN (4 bits),
/// offset low (8 bits) — giving 12-bit offset and lengths 3..=18 in just two
/// bytes.  For simplicity we use a 3-byte encoding: 2 bytes little-endian
/// offset, 1 byte (length - MIN_MATCH_LEN).
pub fn encode_tokens(tokens: &[Token]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let block_len = if tokens.len() - i < 8 { tokens.len() - i } else { 8 };

        // Build flags byte.
        let mut flags: u8 = 0;
        for bit in 0..block_len {
            if let Token::Match { .. } = tokens[i + bit] {
                flags |= 1 << bit;
            }
        }
        out.push(flags);

        // Emit token payloads.
        for bit in 0..block_len {
            match tokens[i + bit] {
                Token::Literal(b) => {
                    out.push(b);
                }
                Token::Match { offset, length } => {
                    out.push(offset as u8);          // low byte
                    out.push((offset >> 8) as u8);   // high byte
                    out.push((length - MIN_MATCH_LEN as u16) as u8);
                }
            }
        }

        i += block_len;
    }

    out
}

/// Decode a binary buffer (produced by [`encode_tokens`]) back into a token
/// stream.
pub fn decode_tokens(data: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos: usize = 0;

    while pos < data.len() {
        let flags = data[pos];
        pos += 1;

        for bit in 0u8..8 {
            if pos >= data.len() {
                break;
            }
            if flags & (1 << bit) != 0 {
                // Match — 3 bytes
                if pos + 2 >= data.len() {
                    break;
                }
                let offset = data[pos] as u16 | ((data[pos + 1] as u16) << 8);
                let length = data[pos + 2] as u16 + MIN_MATCH_LEN as u16;
                tokens.push(Token::Match { offset, length });
                pos += 3;
            } else {
                // Literal — 1 byte
                tokens.push(Token::Literal(data[pos]));
                pos += 1;
            }
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// High-level compress / decompress
// ---------------------------------------------------------------------------

/// Compress `data` using LZ77 with a 4096-byte sliding window.
///
/// Returns the compressed byte stream.  Use [`decompress`] to recover the
/// original data.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let tokens = tokenise(data);
    encode_tokens(&tokens)
}

/// Decompress a byte stream previously produced by [`compress`].
///
/// Reconstructs the original data by replaying literals and back-references.
pub fn decompress(data: &[u8]) -> Vec<u8> {
    let tokens = decode_tokens(data);
    let mut out = Vec::new();

    for token in &tokens {
        match *token {
            Token::Literal(b) => {
                out.push(b);
            }
            Token::Match { offset, length } => {
                let start = out.len().wrapping_sub(offset as usize);
                for i in 0..length as usize {
                    // Index from `out` one byte at a time so that overlapping
                    // copies (offset < length) work correctly — the newly
                    // written bytes feed subsequent reads.
                    let byte = out[start + i];
                    out.push(byte);
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return the compression ratio as a percentage.
///
/// A value of 75.0 means the compressed data is 75 % of the original size
/// (i.e. 25 % space savings).  Returns 0.0 when `original_len` is zero.
pub fn compression_ratio(original_len: usize, compressed_len: usize) -> f32 {
    if original_len == 0 {
        return 0.0;
    }
    (compressed_len as f32 / original_len as f32) * 100.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let data: &[u8] = b"";
        assert_eq!(decompress(&compress(data)), data);
    }

    #[test]
    fn round_trip_short() {
        let data = b"hello";
        assert_eq!(decompress(&compress(data)), data);
    }

    #[test]
    fn round_trip_repetitive() {
        let data = b"abcabcabcabcabcabcabcabcabcabcabc";
        let compressed = compress(data);
        assert_eq!(decompress(&compressed), data);
        // Repetitive data should actually shrink.
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn round_trip_all_same_byte() {
        let data = [0xAA_u8; 512];
        let compressed = compress(&data);
        assert_eq!(decompress(&compressed), &data[..]);
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn compression_ratio_zero_original() {
        assert_eq!(compression_ratio(0, 0), 0.0);
    }

    #[test]
    fn compression_ratio_half() {
        let ratio = compression_ratio(200, 100);
        assert!((ratio - 50.0).abs() < f32::EPSILON);
    }
}
