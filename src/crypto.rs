/// Cryptographic primitives for MerlionOS.
/// Software SHA-256, HMAC-SHA256, XOR cipher, PRNG, and hex formatting.
/// `#![no_std]`-compatible; uses `alloc` for heap-backed return values.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// First 32 bits of the fractional parts of the cube roots of the first 64
/// primes (2..311). Used as round constants in the SHA-256 compression loop.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Initial hash values — first 32 bits of the fractional parts of the square
/// roots of the first 8 primes.
const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Right-rotate a 32-bit word by `n` bits.
#[inline(always)]
fn rotr(x: u32, n: u32) -> u32 {
    (x >> n) | (x << (32 - n))
}

/// SHA-256 Ch function: choose — for each bit position, if `x` then `y` else
/// `z`.
#[inline(always)]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

/// SHA-256 Maj function: majority — for each bit position, produce the
/// majority value of `x`, `y`, `z`.
#[inline(always)]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (x & z) ^ (y & z)
}

/// SHA-256 upper-case Sigma_0 (used in the compression function).
#[inline(always)]
fn big_sigma0(x: u32) -> u32 {
    rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)
}

/// SHA-256 upper-case Sigma_1 (used in the compression function).
#[inline(always)]
fn big_sigma1(x: u32) -> u32 {
    rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)
}

/// SHA-256 lower-case sigma_0 (used in the message schedule).
#[inline(always)]
fn small_sigma0(x: u32) -> u32 {
    rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3)
}

/// SHA-256 lower-case sigma_1 (used in the message schedule).
#[inline(always)]
fn small_sigma1(x: u32) -> u32 {
    rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10)
}

/// Pad `data` according to SHA-256 rules: append bit `1`, then zeros, then
/// the 64-bit big-endian bit length so total length is a multiple of 64 bytes.
fn sha256_pad(data: &[u8]) -> Vec<u8> {
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut buf = Vec::with_capacity(data.len() + 72);
    buf.extend_from_slice(data);
    // Append the 0x80 byte (bit "1" followed by seven zero bits).
    buf.push(0x80);
    // Append zero bytes until length % 64 == 56.
    while buf.len() % 64 != 56 {
        buf.push(0x00);
    }
    // Append the original message length in bits as a 64-bit big-endian int.
    buf.extend_from_slice(&bit_len.to_be_bytes());
    buf
}

/// Process a single 64-byte (512-bit) block, updating `state` in place.
fn sha256_compress(state: &mut [u32; 8], block: &[u8]) {
    // Build the message schedule W[0..64].
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[4 * i],
            block[4 * i + 1],
            block[4 * i + 2],
            block[4 * i + 3],
        ]);
    }
    for i in 16..64 {
        w[i] = small_sigma1(w[i - 2])
            .wrapping_add(w[i - 7])
            .wrapping_add(small_sigma0(w[i - 15]))
            .wrapping_add(w[i - 16]);
    }

    // Initialise working variables from current hash state.
    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    // 64 rounds of compression.
    for i in 0..64 {
        let t1 = h
            .wrapping_add(big_sigma1(e))
            .wrapping_add(ch(e, f, g))
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let t2 = big_sigma0(a).wrapping_add(maj(a, b, c));

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    // Add the compressed chunk to the running hash.
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// Compute the SHA-256 digest of `data`, returning a 32-byte hash.
/// Full FIPS-180-4 compliant: proper padding, 64 round constants, 64-entry
/// message schedule per block.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let padded = sha256_pad(data);
    let mut state = H_INIT;

    // Process each 64-byte block.
    for chunk in padded.chunks_exact(64) {
        sha256_compress(&mut state, chunk);
    }

    // Serialise the 8 × 32-bit state words into 32 bytes (big-endian).
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        let bytes = word.to_be_bytes();
        out[4 * i] = bytes[0];
        out[4 * i + 1] = bytes[1];
        out[4 * i + 2] = bytes[2];
        out[4 * i + 3] = bytes[3];
    }
    out
}

/// HMAC block size for SHA-256 (512 bits = 64 bytes).
const HMAC_BLOCK_SIZE: usize = 64;

/// Compute HMAC-SHA-256 (RFC 2104). Keys longer than 64 bytes are first
/// hashed. Returns a 32-byte MAC.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    // Step 1 — normalise the key to exactly HMAC_BLOCK_SIZE bytes.
    let mut key_block = [0u8; HMAC_BLOCK_SIZE];
    if key.len() > HMAC_BLOCK_SIZE {
        let hashed = sha256(key);
        key_block[..32].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    // Step 2 — build inner and outer padded keys.
    let mut i_key_pad = [0x36u8; HMAC_BLOCK_SIZE];
    let mut o_key_pad = [0x5cu8; HMAC_BLOCK_SIZE];
    for i in 0..HMAC_BLOCK_SIZE {
        i_key_pad[i] ^= key_block[i];
        o_key_pad[i] ^= key_block[i];
    }

    // Step 3 — inner hash: H(i_key_pad || data).
    let mut inner_msg = Vec::with_capacity(HMAC_BLOCK_SIZE + data.len());
    inner_msg.extend_from_slice(&i_key_pad);
    inner_msg.extend_from_slice(data);
    let inner_hash = sha256(&inner_msg);

    // Step 4 — outer hash: H(o_key_pad || inner_hash).
    let mut outer_msg = Vec::with_capacity(HMAC_BLOCK_SIZE + 32);
    outer_msg.extend_from_slice(&o_key_pad);
    outer_msg.extend_from_slice(&inner_hash);

    sha256(&outer_msg)
}

/// Encrypt (or decrypt) `data` by repeating `key` via XOR. Applying the
/// same key twice recovers the original. Empty key returns data unchanged.
pub fn xor_cipher(data: &[u8], key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    let mut out = vec![0u8; data.len()];
    for (i, &byte) in data.iter().enumerate() {
        out[i] = byte ^ key[i % key.len()];
    }
    out
}

/// Read the x86 Time-Stamp Counter (RDTSC) as a 64-bit value.
/// Increments every CPU clock cycle; primary entropy source.
#[inline(always)]
fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
    // Fallback for non-x86 builds (e.g. host-side testing on aarch64).
    #[cfg(not(target_arch = "x86_64"))]
    {
        0xcafe_dead_beef_1234u64
    }
}

/// Xorshift64-star mixing function to diffuse entropy across all bits.
#[inline(always)]
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

/// Fill `buf` with pseudo-random bytes seeded from RDTSC and a monotonic
/// counter. Not cryptographically secure — suitable for nonces, ASLR,
/// and test data.
pub fn random_bytes(buf: &mut [u8]) {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Monotonic counter — ensures successive calls never reuse state even if
    /// RDTSC returns the same value (e.g. under aggressive virtualisation).
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut state = rdtsc();

    for byte in buf.iter_mut() {
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        state = mix64(state ^ count);
        *byte = (state & 0xff) as u8;
        // Re-sample RDTSC every 8 bytes to inject fresh entropy without
        // paying the cost of an `rdtsc` instruction on every iteration.
        if count & 0x07 == 0 {
            state ^= rdtsc();
        }
    }
}

/// Format a byte slice as a lowercase hexadecimal string.
pub fn hex_digest(hash: &[u8]) -> String {
    use core::fmt::Write;
    let mut s = String::with_capacity(hash.len() * 2);
    for &b in hash {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty() {
        let d = sha256(b"");
        assert_eq!(hex_digest(&d), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn sha256_abc() {
        let d = sha256(b"abc");
        assert_eq!(hex_digest(&d), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn sha256_multiblock() {
        let d = sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(hex_digest(&d), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn hmac_rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex_digest(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn xor_roundtrip() {
        let ct = xor_cipher(b"MerlionOS", b"key");
        assert_eq!(xor_cipher(&ct, b"key"), b"MerlionOS");
    }

    #[test]
    fn xor_empty_key() {
        assert_eq!(xor_cipher(b"data", b""), b"data".to_vec());
    }

    #[test]
    fn random_bytes_nonzero() {
        let mut buf = [0u8; 64];
        random_bytes(&mut buf);
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn hex_digest_format() {
        assert_eq!(hex_digest(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
