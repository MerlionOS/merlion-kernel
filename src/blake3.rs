/// Blake3 cryptographic hash for MerlionOS.
/// Pure Rust no_std implementation of the BLAKE3 hash function.
/// Used by QFC blockchain for block hashing and proof-of-work.

use alloc::string::String;

// ── Constants ───────────────────────────────────────────────────────

/// BLAKE3 initialization vector (same as SHA-256 IV).
const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
    0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

/// Message word permutation schedule for each round.
const MSG_PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

/// Block size in bytes (64 bytes = 16 x u32 words).
const BLOCK_LEN: usize = 64;

/// Chunk size in bytes (1024 bytes = 16 blocks).
const CHUNK_LEN: usize = 1024;

// ── Block flags ─────────────────────────────────────────────────────

const CHUNK_START: u32 = 1;
const CHUNK_END: u32 = 2;
const PARENT: u32 = 4;
const ROOT: u32 = 8;
const KEYED_HASH: u32 = 16;

// ── G mixing function ───────────────────────────────────────────────

/// The quarter-round mixing function G used in each round.
#[inline(always)]
fn g(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(mx);
    state[d] = (state[d] ^ state[a]).rotate_right(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(12);
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(my);
    state[d] = (state[d] ^ state[a]).rotate_right(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(7);
}

/// Perform one round of mixing on the 4x4 state matrix.
fn round(state: &mut [u32; 16], m: &[u32; 16]) {
    // Column step
    g(state, 0, 4,  8, 12, m[0], m[1]);
    g(state, 1, 5,  9, 13, m[2], m[3]);
    g(state, 2, 6, 10, 14, m[4], m[5]);
    g(state, 3, 7, 11, 15, m[6], m[7]);
    // Diagonal step
    g(state, 0, 5, 10, 15, m[8],  m[9]);
    g(state, 1, 6, 11, 12, m[10], m[11]);
    g(state, 2, 7,  8, 13, m[12], m[13]);
    g(state, 3, 4,  9, 14, m[14], m[15]);
}

/// Permute message words for the next round.
fn permute(m: &[u32; 16]) -> [u32; 16] {
    let mut permuted = [0u32; 16];
    for i in 0..16 {
        permuted[i] = m[MSG_PERMUTATION[i]];
    }
    permuted
}

// ── Compression function ────────────────────────────────────────────

/// BLAKE3 compression function.  Takes a chaining value (8 words), a block of
/// 16 message words, a counter, block length, and flags.  Returns the full
/// 16-word state after compression.
fn compress(
    chaining_value: &[u32; 8],
    block_words: &[u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u32; 16] {
    let mut state = [0u32; 16];
    // First four rows from chaining value
    state[0] = chaining_value[0];
    state[1] = chaining_value[1];
    state[2] = chaining_value[2];
    state[3] = chaining_value[3];
    state[4] = chaining_value[4];
    state[5] = chaining_value[5];
    state[6] = chaining_value[6];
    state[7] = chaining_value[7];
    // Third row from IV constants
    state[8]  = IV[0];
    state[9]  = IV[1];
    state[10] = IV[2];
    state[11] = IV[3];
    // Fourth row from counter, block_len, flags
    state[12] = counter as u32;
    state[13] = (counter >> 32) as u32;
    state[14] = block_len;
    state[15] = flags;

    let mut m = *block_words;

    // 7 rounds of mixing
    round(&mut state, &m); m = permute(&m);
    round(&mut state, &m); m = permute(&m);
    round(&mut state, &m); m = permute(&m);
    round(&mut state, &m); m = permute(&m);
    round(&mut state, &m); m = permute(&m);
    round(&mut state, &m); m = permute(&m);
    round(&mut state, &m);

    // Feed-forward XOR
    for i in 0..8 {
        state[i] ^= state[i + 8];
    }
    for i in 8..16 {
        state[i] ^= chaining_value[i - 8];
    }

    state
}

/// Extract first 8 words from compression output as chaining value.
fn first_8(state: &[u32; 16]) -> [u32; 8] {
    let mut cv = [0u32; 8];
    cv.copy_from_slice(&state[..8]);
    cv
}

// ── Helper: bytes to words ──────────────────────────────────────────

/// Read a little-endian u32 from a byte slice at `offset`.
#[inline(always)]
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    let b = &data[offset..offset + 4];
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Convert a 64-byte block into 16 message words (little-endian).
fn block_to_words(block: &[u8]) -> [u32; 16] {
    let mut words = [0u32; 16];
    for i in 0..16 {
        if i * 4 + 4 <= block.len() {
            words[i] = read_u32_le(block, i * 4);
        } else if i * 4 < block.len() {
            // Partial last word — zero-pad
            let mut buf = [0u8; 4];
            let remaining = block.len() - i * 4;
            buf[..remaining].copy_from_slice(&block[i * 4..]);
            words[i] = u32::from_le_bytes(buf);
        }
    }
    words
}

/// Convert a chaining value (8 words) to 32 bytes.
fn cv_to_bytes(cv: &[u32; 8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..8 {
        let b = cv[i].to_le_bytes();
        out[i * 4..i * 4 + 4].copy_from_slice(&b);
    }
    out
}

// ── Chunk processing ────────────────────────────────────────────────

/// Process a single chunk (up to 1024 bytes) and return its 32-byte chaining
/// value.  `chunk_counter` is the zero-based index of this chunk.
fn process_chunk(
    key_words: &[u32; 8],
    chunk_data: &[u8],
    chunk_counter: u64,
    flags: u32,
) -> [u32; 8] {
    let mut cv = *key_words;
    let num_blocks = if chunk_data.is_empty() {
        1
    } else {
        (chunk_data.len() + BLOCK_LEN - 1) / BLOCK_LEN
    };

    for i in 0..num_blocks {
        let start = i * BLOCK_LEN;
        let end = if start + BLOCK_LEN <= chunk_data.len() {
            start + BLOCK_LEN
        } else {
            chunk_data.len()
        };
        let block = &chunk_data[start..end];
        let block_words = block_to_words(block);
        let block_len = (end - start) as u32;

        let mut block_flags = flags;
        if i == 0 {
            block_flags |= CHUNK_START;
        }
        if i == num_blocks - 1 {
            block_flags |= CHUNK_END;
        }

        let state = compress(&cv, &block_words, chunk_counter, block_len, block_flags);
        cv = first_8(&state);
    }

    cv
}

/// Compute the root output from a chaining value (used for single-chunk or
/// final parent).
fn root_output(
    key_words: &[u32; 8],
    block_words: &[u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u8; 32] {
    let state = compress(key_words, block_words, counter, block_len, flags | ROOT);
    let cv = first_8(&state);
    cv_to_bytes(&cv)
}

/// Compress two chaining values (parent node) and return the parent chaining
/// value.
fn parent_cv(
    left: &[u32; 8],
    right: &[u32; 8],
    key_words: &[u32; 8],
    flags: u32,
) -> [u32; 8] {
    let mut block = [0u32; 16];
    block[..8].copy_from_slice(left);
    block[8..].copy_from_slice(right);
    let state = compress(key_words, &block, 0, BLOCK_LEN as u32, flags | PARENT);
    first_8(&state)
}

/// Convert 32 bytes to 8 words.
fn bytes_to_cv(bytes: &[u8; 32]) -> [u32; 8] {
    let mut cv = [0u32; 8];
    for i in 0..8 {
        cv[i] = read_u32_le(bytes, i * 4);
    }
    cv
}

// ── Public API ──────────────────────────────────────────────────────

/// Hash arbitrary data using BLAKE3 and return a 32-byte digest.
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    blake3_internal(data, &IV, 0)
}

/// Hash two 32-byte values together (for Merkle tree parent hashing).
pub fn blake3_hash_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let left = bytes_to_cv(a);
    let right = bytes_to_cv(b);

    let mut block = [0u32; 16];
    block[..8].copy_from_slice(&left);
    block[8..].copy_from_slice(&right);

    let state = compress(&IV, &block, 0, BLOCK_LEN as u32, PARENT | ROOT);
    let cv = first_8(&state);
    cv_to_bytes(&cv)
}

/// Keyed hash (MAC) using a 32-byte key.
pub fn blake3_keyed_hash(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let key_words = bytes_to_cv(key);
    blake3_internal(data, &key_words, KEYED_HASH)
}

/// Format a 32-byte hash as a 64-character lowercase hex string.
pub fn blake3_hex(hash: &[u8; 32]) -> String {
    use core::fmt::Write;
    let mut s = String::with_capacity(64);
    for &b in hash.iter() {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Core BLAKE3 implementation shared by `blake3_hash` and `blake3_keyed_hash`.
fn blake3_internal(data: &[u8], key_words: &[u32; 8], flags: u32) -> [u8; 32] {
    if data.is_empty() {
        // Hash of empty input — process a single empty chunk and finalize
        let block_words = [0u32; 16];
        return root_output(
            key_words,
            &block_words,
            0,
            0,
            flags | CHUNK_START | CHUNK_END,
        );
    }

    let num_chunks = (data.len() + CHUNK_LEN - 1) / CHUNK_LEN;

    if num_chunks == 1 {
        // Single chunk — process and finalize as root
        let chunk = data;
        let num_blocks = (chunk.len() + BLOCK_LEN - 1) / BLOCK_LEN;
        let mut cv = *key_words;

        // Process all blocks except the last one normally
        for i in 0..num_blocks {
            let start = i * BLOCK_LEN;
            let end = if start + BLOCK_LEN <= chunk.len() {
                start + BLOCK_LEN
            } else {
                chunk.len()
            };
            let block = &chunk[start..end];
            let block_words = block_to_words(block);
            let block_len = (end - start) as u32;

            let mut block_flags = flags;
            if i == 0 {
                block_flags |= CHUNK_START;
            }
            if i == num_blocks - 1 {
                block_flags |= CHUNK_END;
                // Last block of single chunk — finalize as root
                return root_output(&cv, &block_words, 0, block_len, block_flags);
            }

            let state = compress(&cv, &block_words, 0, block_len, block_flags);
            cv = first_8(&state);
        }

        // Shouldn't reach here, but just in case
        cv_to_bytes(&cv)
    } else {
        // Multiple chunks — build Merkle tree
        // First, compute chaining values for all chunks
        let mut cvs: alloc::vec::Vec<[u32; 8]> = alloc::vec::Vec::with_capacity(num_chunks);

        for chunk_idx in 0..num_chunks {
            let start = chunk_idx * CHUNK_LEN;
            let end = if start + CHUNK_LEN <= data.len() {
                start + CHUNK_LEN
            } else {
                data.len()
            };
            let chunk_data = &data[start..end];
            let cv = process_chunk(key_words, chunk_data, chunk_idx as u64, flags);
            cvs.push(cv);
        }

        // Merge chaining values into parent nodes until we have one root
        while cvs.len() > 2 {
            let mut next: alloc::vec::Vec<[u32; 8]> = alloc::vec::Vec::with_capacity((cvs.len() + 1) / 2);
            let mut i = 0;
            while i + 1 < cvs.len() {
                let pcv = parent_cv(&cvs[i], &cvs[i + 1], key_words, flags);
                next.push(pcv);
                i += 2;
            }
            if i < cvs.len() {
                next.push(cvs[i]);
            }
            cvs = next;
        }

        if cvs.len() == 1 {
            // Odd case: only one CV left — finalize
            cv_to_bytes(&cvs[0])
        } else {
            // Two CVs left — compute root parent
            let mut block = [0u32; 16];
            block[..8].copy_from_slice(&cvs[0]);
            block[8..].copy_from_slice(&cvs[1]);
            root_output(key_words, &block, 0, BLOCK_LEN as u32, flags | PARENT)
        }
    }
}

// ── Init / Info ─────────────────────────────────────────────────────

/// Initialize the BLAKE3 module (no-op, stateless).
pub fn init() {
    // BLAKE3 is stateless — nothing to initialize
}

/// Return a summary of the BLAKE3 module.
pub fn blake3_info() -> String {
    alloc::format!(
        "BLAKE3 hash: 256-bit output, {} byte chunks, 7 rounds, no_std pure Rust",
        CHUNK_LEN
    )
}
