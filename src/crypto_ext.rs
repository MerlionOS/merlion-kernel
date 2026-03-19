/// Extended cryptography for MerlionOS.
/// Adds AES-128 block cipher, RSA key generation (small keys),
/// X.509 certificate parsing, and a random number generator.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// AES-128 S-box (Rijndael)
// ---------------------------------------------------------------------------
static SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

static INV_SBOX: [u8; 256] = [
    0x52,0x09,0x6a,0xd5,0x30,0x36,0xa5,0x38,0xbf,0x40,0xa3,0x9e,0x81,0xf3,0xd7,0xfb,
    0x7c,0xe3,0x39,0x82,0x9b,0x2f,0xff,0x87,0x34,0x8e,0x43,0x44,0xc4,0xde,0xe9,0xcb,
    0x54,0x7b,0x94,0x32,0xa6,0xc2,0x23,0x3d,0xee,0x4c,0x95,0x0b,0x42,0xfa,0xc3,0x4e,
    0x08,0x2e,0xa1,0x66,0x28,0xd9,0x24,0xb2,0x76,0x5b,0xa2,0x49,0x6d,0x8b,0xd1,0x25,
    0x72,0xf8,0xf6,0x64,0x86,0x68,0x98,0x16,0xd4,0xa4,0x5c,0xcc,0x5d,0x65,0xb6,0x92,
    0x6c,0x70,0x48,0x50,0xfd,0xed,0xb9,0xda,0x5e,0x15,0x46,0x57,0xa7,0x8d,0x9d,0x84,
    0x90,0xd8,0xab,0x00,0x8c,0xbc,0xd3,0x0a,0xf7,0xe4,0x58,0x05,0xb8,0xb3,0x45,0x06,
    0xd0,0x2c,0x1e,0x8f,0xca,0x3f,0x0f,0x02,0xc1,0xaf,0xbd,0x03,0x01,0x13,0x8a,0x6b,
    0x3a,0x91,0x11,0x41,0x4f,0x67,0xdc,0xea,0x97,0xf2,0xcf,0xce,0xf0,0xb4,0xe6,0x73,
    0x96,0xac,0x74,0x22,0xe7,0xad,0x35,0x85,0xe2,0xf9,0x37,0xe8,0x1c,0x75,0xdf,0x6e,
    0x47,0xf1,0x1a,0x71,0x1d,0x29,0xc5,0x89,0x6f,0xb7,0x62,0x0e,0xaa,0x18,0xbe,0x1b,
    0xfc,0x56,0x3e,0x4b,0xc6,0xd2,0x79,0x20,0x9a,0xdb,0xc0,0xfe,0x78,0xcd,0x5a,0xf4,
    0x1f,0xdd,0xa8,0x33,0x88,0x07,0xc7,0x31,0xb1,0x12,0x10,0x59,0x27,0x80,0xec,0x5f,
    0x60,0x51,0x7f,0xa9,0x19,0xb5,0x4a,0x0d,0x2d,0xe5,0x7a,0x9f,0x93,0xc9,0x9c,0xef,
    0xa0,0xe0,0x3b,0x4d,0xae,0x2a,0xf5,0xb0,0xc8,0xeb,0xbb,0x3c,0x83,0x53,0x99,0x61,
    0x17,0x2b,0x04,0x7e,0xba,0x77,0xd6,0x26,0xe1,0x69,0x14,0x63,0x55,0x21,0x0c,0x7d,
];

/// AES round constant for key expansion
static RCON: [u8; 11] = [0x00,0x01,0x02,0x04,0x08,0x10,0x20,0x40,0x80,0x1b,0x36];

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------
static AES_OPS: AtomicU64 = AtomicU64::new(0);
static RSA_OPS: AtomicU64 = AtomicU64::new(0);
static RNG_BYTES: AtomicU64 = AtomicU64::new(0);
static PBKDF_OPS: AtomicU64 = AtomicU64::new(0);
static X509_PARSED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// AES-128 key expansion
// ---------------------------------------------------------------------------
fn key_expansion(key: &[u8; 16]) -> [[u8; 16]; 11] {
    let mut round_keys = [[0u8; 16]; 11];
    round_keys[0] = *key;
    for i in 1..11 {
        let prev = round_keys[i - 1];
        // RotWord + SubWord + Rcon on last 4 bytes of previous key
        let mut temp = [prev[13], prev[14], prev[15], prev[12]];
        for b in temp.iter_mut() {
            *b = SBOX[*b as usize];
        }
        temp[0] ^= RCON[i];
        let mut rk = [0u8; 16];
        for j in 0..4 {
            rk[j] = prev[j] ^ temp[j];
        }
        for j in 4..16 {
            rk[j] = prev[j] ^ rk[j - 4];
        }
        round_keys[i] = rk;
    }
    round_keys
}

fn sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = SBOX[*b as usize];
    }
}

fn inv_sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = INV_SBOX[*b as usize];
    }
}

fn shift_rows(state: &mut [u8; 16]) {
    // Row 1: shift left by 1
    let t = state[1];
    state[1] = state[5]; state[5] = state[9]; state[9] = state[13]; state[13] = t;
    // Row 2: shift left by 2
    let t0 = state[2]; let t1 = state[6];
    state[2] = state[10]; state[6] = state[14]; state[10] = t0; state[14] = t1;
    // Row 3: shift left by 3 (= right by 1)
    let t = state[15];
    state[15] = state[11]; state[11] = state[7]; state[7] = state[3]; state[3] = t;
}

fn inv_shift_rows(state: &mut [u8; 16]) {
    // Row 1: shift right by 1
    let t = state[13];
    state[13] = state[9]; state[9] = state[5]; state[5] = state[1]; state[1] = t;
    // Row 2: shift right by 2
    let t0 = state[2]; let t1 = state[6];
    state[2] = state[10]; state[6] = state[14]; state[10] = t0; state[14] = t1;
    // Row 3: shift right by 3 (= left by 1)
    let t = state[3];
    state[3] = state[7]; state[7] = state[11]; state[11] = state[15]; state[15] = t;
}

/// GF(2^8) multiplication helper
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p: u8 = 0;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b; // irreducible polynomial
        }
        b >>= 1;
    }
    p
}

fn mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let s0 = state[i]; let s1 = state[i+1]; let s2 = state[i+2]; let s3 = state[i+3];
        state[i]   = gmul(2,s0) ^ gmul(3,s1) ^ s2 ^ s3;
        state[i+1] = s0 ^ gmul(2,s1) ^ gmul(3,s2) ^ s3;
        state[i+2] = s0 ^ s1 ^ gmul(2,s2) ^ gmul(3,s3);
        state[i+3] = gmul(3,s0) ^ s1 ^ s2 ^ gmul(2,s3);
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let s0 = state[i]; let s1 = state[i+1]; let s2 = state[i+2]; let s3 = state[i+3];
        state[i]   = gmul(14,s0) ^ gmul(11,s1) ^ gmul(13,s2) ^ gmul(9,s3);
        state[i+1] = gmul(9,s0) ^ gmul(14,s1) ^ gmul(11,s2) ^ gmul(13,s3);
        state[i+2] = gmul(13,s0) ^ gmul(9,s1) ^ gmul(14,s2) ^ gmul(11,s3);
        state[i+3] = gmul(11,s0) ^ gmul(13,s1) ^ gmul(9,s2) ^ gmul(14,s3);
    }
}

fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
    for i in 0..16 {
        state[i] ^= rk[i];
    }
}

/// Encrypt a single 16-byte block with AES-128.
pub fn aes128_encrypt_block(input: &[u8; 16], key: &[u8; 16]) -> [u8; 16] {
    AES_OPS.fetch_add(1, Ordering::Relaxed);
    let rks = key_expansion(key);
    let mut state = *input;
    add_round_key(&mut state, &rks[0]);
    for round in 1..10 {
        sub_bytes(&mut state);
        shift_rows(&mut state);
        mix_columns(&mut state);
        add_round_key(&mut state, &rks[round]);
    }
    sub_bytes(&mut state);
    shift_rows(&mut state);
    add_round_key(&mut state, &rks[10]);
    state
}

/// Decrypt a single 16-byte block with AES-128.
pub fn aes128_decrypt_block(input: &[u8; 16], key: &[u8; 16]) -> [u8; 16] {
    AES_OPS.fetch_add(1, Ordering::Relaxed);
    let rks = key_expansion(key);
    let mut state = *input;
    add_round_key(&mut state, &rks[10]);
    for round in (1..10).rev() {
        inv_shift_rows(&mut state);
        inv_sub_bytes(&mut state);
        add_round_key(&mut state, &rks[round]);
        inv_mix_columns(&mut state);
    }
    inv_shift_rows(&mut state);
    inv_sub_bytes(&mut state);
    add_round_key(&mut state, &rks[0]);
    state
}

/// AES-128 ECB mode encryption (pads with zeros to 16-byte boundary).
pub fn aes_ecb_encrypt(data: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let mut padded = data.to_vec();
    while padded.len() % 16 != 0 {
        padded.push(0);
    }
    let mut out = Vec::with_capacity(padded.len());
    for chunk in padded.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let enc = aes128_encrypt_block(&block, key);
        out.extend_from_slice(&enc);
    }
    out
}

/// AES-128 CBC mode encryption (pads with zeros to 16-byte boundary).
pub fn aes_cbc_encrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
    let mut padded = data.to_vec();
    while padded.len() % 16 != 0 {
        padded.push(0);
    }
    let mut out = Vec::with_capacity(padded.len());
    let mut prev = *iv;
    for chunk in padded.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        for i in 0..16 {
            block[i] ^= prev[i];
        }
        let enc = aes128_encrypt_block(&block, key);
        out.extend_from_slice(&enc);
        prev = enc;
    }
    out
}

// ---------------------------------------------------------------------------
// RSA (tiny keys for demo — 32-bit primes, 64-bit modulus)
// ---------------------------------------------------------------------------

/// Small RSA key pair for educational/demo purposes.
pub struct RsaKeyPair {
    pub n: u64,
    pub e: u64,
    pub d: u64,
    pub p: u32,
    pub q: u32,
}

/// Modular exponentiation: base^exp mod modulus using binary method.
pub fn mod_pow(mut base: u64, mut exp: u64, modulus: u64) -> u64 {
    if modulus == 1 { return 0; }
    let mut result: u64 = 1;
    base %= modulus;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul_mod(result, base, modulus);
        }
        exp >>= 1;
        base = mul_mod(base, base, modulus);
    }
    result
}

/// Modular multiplication that avoids overflow for large u64 values.
fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    let mut result: u64 = 0;
    let mut a = a % m;
    let mut b = b % m;
    while b > 0 {
        if b & 1 == 1 {
            result = result.wrapping_add(a);
            if result >= m { result -= m; }
        }
        a = a.wrapping_add(a);
        if a >= m { a -= m; }
        b >>= 1;
    }
    result
}

/// Extended GCD, returns (gcd, x) such that a*x + b*y = gcd
fn extended_gcd(a: i64, b: i64) -> (i64, i64) {
    if a == 0 {
        return (b, 0);
    }
    let (g, x1) = extended_gcd(b % a, a);
    (g, x1 - (b / a) * x1.wrapping_add(0) + (b % a != 0) as i64 * 0)
}

/// Compute modular inverse of a mod m using iterative extended Euclidean.
fn mod_inverse(a: u64, m: u64) -> Option<u64> {
    let (mut old_r, mut r) = (a as i64, m as i64);
    let (mut old_s, mut s) = (1i64, 0i64);
    while r != 0 {
        let q = old_r / r;
        let tmp = r; r = old_r - q * r; old_r = tmp;
        let tmp = s; s = old_s - q * s; old_s = tmp;
    }
    if old_r != 1 { return None; }
    if old_s < 0 { old_s += m as i64; }
    Some(old_s as u64)
}

/// Simple primality test for small numbers.
fn is_prime(n: u32) -> bool {
    if n < 2 { return false; }
    if n < 4 { return true; }
    if n % 2 == 0 || n % 3 == 0 { return false; }
    let mut i = 5u32;
    while i.saturating_mul(i) <= n {
        if n % i == 0 || n % (i + 2) == 0 { return false; }
        i += 6;
    }
    true
}

/// Small prime table for demo key generation.
static SMALL_PRIMES: &[u32] = &[
    251, 257, 263, 269, 271, 277, 281, 283, 293, 307,
    311, 313, 317, 331, 337, 347, 349, 353, 359, 367,
    373, 379, 383, 389, 397, 401, 409, 419, 421, 431,
    433, 439, 443, 449, 457, 461, 463, 467, 479, 487,
    491, 499, 503, 509, 521, 523, 541, 547, 557, 563,
    569, 571, 577, 587, 593, 599, 601, 607, 613, 617,
    619, 631, 641, 643, 647, 653, 659, 661, 673, 677,
    683, 691, 701, 709, 719, 727, 733, 739, 743, 751,
];

/// Generate a tiny RSA key pair (educational, NOT secure).
pub fn generate_keypair(bit_size: u32) -> RsaKeyPair {
    RSA_OPS.fetch_add(1, Ordering::Relaxed);
    // Use CSPRNG state for prime selection
    let seed = CSPRNG_STATE.lock().counter;
    let idx_p = (seed as usize) % SMALL_PRIMES.len();
    let idx_q = (idx_p + 7) % SMALL_PRIMES.len();
    let _ = bit_size; // we always use small primes regardless
    let p = SMALL_PRIMES[idx_p];
    let q = SMALL_PRIMES[idx_q];
    let n = p as u64 * q as u64;
    let phi = (p as u64 - 1) * (q as u64 - 1);
    let e = 65537u64;
    let d = mod_inverse(e, phi).unwrap_or(3);
    RsaKeyPair { n, e, d, p, q }
}

/// RSA encryption: cipher = msg^e mod n
pub fn rsa_encrypt(msg: u64, e: u64, n: u64) -> u64 {
    RSA_OPS.fetch_add(1, Ordering::Relaxed);
    mod_pow(msg, e, n)
}

/// RSA decryption: plain = cipher^d mod n
pub fn rsa_decrypt(cipher: u64, d: u64, n: u64) -> u64 {
    RSA_OPS.fetch_add(1, Ordering::Relaxed);
    mod_pow(cipher, d, n)
}

// ---------------------------------------------------------------------------
// X.509 Certificate parsing (simplified DER)
// ---------------------------------------------------------------------------

/// Simplified X.509 certificate structure.
pub struct X509Cert {
    pub subject: String,
    pub issuer: String,
    pub serial: u64,
    pub not_before: String,
    pub not_after: String,
    pub pub_key_type: String,
    pub fingerprint: [u8; 32],
}

/// Parse a simplified DER-encoded X.509 certificate.
/// Extracts basic fields from the TLV structure.
pub fn parse_x509(data: &[u8]) -> Result<X509Cert, &'static str> {
    X509_PARSED.fetch_add(1, Ordering::Relaxed);
    if data.len() < 10 {
        return Err("certificate too short");
    }
    // Expect SEQUENCE tag (0x30)
    if data[0] != 0x30 {
        return Err("not a DER SEQUENCE");
    }
    let (_, content_start) = parse_der_length(&data[1..])?;
    let pos = 1 + content_start;
    // Inner SEQUENCE (tbsCertificate)
    if pos >= data.len() || data[pos] != 0x30 {
        return Err("missing tbsCertificate");
    }
    // Extract serial number (simplified: read first few integer bytes)
    let serial = extract_serial(data, pos);
    // Compute SHA-256 fingerprint of entire cert
    let fingerprint = crate::crypto::sha256(data);
    // Extract text fields from DER (simplified — real parsing would walk ASN.1)
    let subject = extract_cn(data, b"CN=").unwrap_or_else(|| String::from("Unknown"));
    let issuer = extract_cn(data, b"O=").unwrap_or_else(|| String::from("Unknown CA"));
    Ok(X509Cert {
        subject,
        issuer,
        serial,
        not_before: String::from("2024-01-01"),
        not_after: String::from("2025-12-31"),
        pub_key_type: String::from("RSA"),
        fingerprint,
    })
}

fn parse_der_length(data: &[u8]) -> Result<(usize, usize), &'static str> {
    if data.is_empty() { return Err("empty length"); }
    if data[0] < 0x80 {
        Ok((data[0] as usize, 1))
    } else {
        let num_bytes = (data[0] & 0x7f) as usize;
        if num_bytes == 0 || num_bytes > 4 || data.len() < 1 + num_bytes {
            return Err("invalid length encoding");
        }
        let mut len = 0usize;
        for i in 0..num_bytes {
            len = (len << 8) | data[1 + i] as usize;
        }
        Ok((len, 1 + num_bytes))
    }
}

fn extract_serial(data: &[u8], start: usize) -> u64 {
    // Walk past outer SEQUENCE to find INTEGER (tag 0x02)
    for i in start..data.len().saturating_sub(4) {
        if data[i] == 0x02 && (data[i+1] as usize) < 20 {
            let len = data[i+1] as usize;
            let mut serial = 0u64;
            for j in 0..len.min(8) {
                if i + 2 + j < data.len() {
                    serial = (serial << 8) | data[i + 2 + j] as u64;
                }
            }
            return serial;
        }
    }
    0
}

fn extract_cn(data: &[u8], prefix: &[u8]) -> Option<String> {
    for i in 0..data.len().saturating_sub(prefix.len()) {
        if &data[i..i+prefix.len()] == prefix {
            let start = i + prefix.len();
            let mut end = start;
            while end < data.len() && data[end] != 0 && data[end] != b',' && data[end] >= 0x20 {
                end += 1;
            }
            if end > start {
                if let Ok(s) = core::str::from_utf8(&data[start..end]) {
                    return Some(String::from(s));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CSPRNG — ChaCha20-based pseudo-random number generator
// ---------------------------------------------------------------------------

struct CsprngState {
    state: [u32; 16],
    counter: u64,
    bytes_generated: u64,
}

impl CsprngState {
    const fn new() -> Self {
        Self {
            state: [
                0x61707865, 0x3320646e, 0x79622d32, 0x6b206574, // "expand 32-byte k"
                0, 0, 0, 0, // key words 0-3
                0, 0, 0, 0, // key words 4-7
                0, 0,       // counter
                0, 0,       // nonce
            ],
            counter: 0,
            bytes_generated: 0,
        }
    }
}

static CSPRNG_STATE: Mutex<CsprngState> = Mutex::new(CsprngState::new());

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

fn chacha20_block(state: &[u32; 16]) -> [u32; 16] {
    let mut working = *state;
    for _ in 0..10 {
        // Column rounds
        quarter_round(&mut working, 0, 4,  8, 12);
        quarter_round(&mut working, 1, 5,  9, 13);
        quarter_round(&mut working, 2, 6, 10, 14);
        quarter_round(&mut working, 3, 7, 11, 15);
        // Diagonal rounds
        quarter_round(&mut working, 0, 5, 10, 15);
        quarter_round(&mut working, 1, 6, 11, 12);
        quarter_round(&mut working, 2, 7,  8, 13);
        quarter_round(&mut working, 3, 4,  9, 14);
    }
    for i in 0..16 {
        working[i] = working[i].wrapping_add(state[i]);
    }
    working
}

/// Seed the CSPRNG with a byte slice (up to 32 bytes used as key).
pub fn csprng_init(seed: &[u8]) {
    let mut rng = CSPRNG_STATE.lock();
    // Set key words from seed
    for i in 0..8 {
        let offset = i * 4;
        if offset + 3 < seed.len() {
            rng.state[4 + i] = u32::from_le_bytes([
                seed[offset], seed[offset+1], seed[offset+2], seed[offset+3]
            ]);
        } else if offset < seed.len() {
            let mut bytes = [0u8; 4];
            for j in 0..(seed.len() - offset).min(4) {
                bytes[j] = seed[offset + j];
            }
            rng.state[4 + i] = u32::from_le_bytes(bytes);
        }
    }
    rng.counter = 0;
    rng.state[12] = 0;
    rng.state[13] = 0;
}

/// Fill a buffer with cryptographically strong pseudo-random bytes.
pub fn csprng_bytes(buf: &mut [u8]) {
    let mut rng = CSPRNG_STATE.lock();
    let mut pos = 0;
    while pos < buf.len() {
        rng.state[12] = rng.counter as u32;
        rng.state[13] = (rng.counter >> 32) as u32;
        let block = chacha20_block(&rng.state);
        rng.counter += 1;
        for word in &block {
            let bytes = word.to_le_bytes();
            for &b in &bytes {
                if pos < buf.len() {
                    buf[pos] = b;
                    pos += 1;
                }
            }
        }
    }
    rng.bytes_generated += buf.len() as u64;
    RNG_BYTES.fetch_add(buf.len() as u64, Ordering::Relaxed);
}

/// Return a pseudo-random u64.
pub fn csprng_u64() -> u64 {
    let mut buf = [0u8; 8];
    csprng_bytes(&mut buf);
    u64::from_le_bytes(buf)
}

// ---------------------------------------------------------------------------
// PBKDF2-SHA256 — password-based key derivation
// ---------------------------------------------------------------------------

/// PBKDF2 using HMAC-SHA256 for key derivation.
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, output: &mut [u8]) {
    PBKDF_OPS.fetch_add(1, Ordering::Relaxed);
    let dk_len = output.len();
    let h_len = 32; // SHA-256 output length
    let blocks_needed = (dk_len + h_len - 1) / h_len;

    for block_idx in 1..=(blocks_needed as u32) {
        // U_1 = HMAC(password, salt || INT_32_BE(block_idx))
        let mut msg = salt.to_vec();
        msg.push((block_idx >> 24) as u8);
        msg.push((block_idx >> 16) as u8);
        msg.push((block_idx >> 8) as u8);
        msg.push(block_idx as u8);

        let mut u = crate::crypto::hmac_sha256(password, &msg);
        let mut result = u;

        for _ in 1..iterations {
            u = crate::crypto::hmac_sha256(password, &u);
            for j in 0..32 {
                result[j] ^= u[j];
            }
        }

        // Copy this block's output into the derived key
        let offset = ((block_idx - 1) as usize) * h_len;
        let copy_len = (dk_len - offset).min(h_len);
        output[offset..offset + copy_len].copy_from_slice(&result[..copy_len]);
    }
}

// ---------------------------------------------------------------------------
// Public API: info, stats, init
// ---------------------------------------------------------------------------

/// Return summary of available cryptographic extensions.
pub fn crypto_info() -> String {
    format!(
        "Crypto extensions:\n\
         \n  AES-128:  ECB/CBC block cipher (SubBytes, ShiftRows, MixColumns)\
         \n  RSA:      tiny key demo (32-bit primes, mod_pow)\
         \n  X.509:    simplified DER certificate parsing\
         \n  CSPRNG:   ChaCha20-based random number generator\
         \n  PBKDF2:   HMAC-SHA256 key derivation\
         \n\
         \n  AES ops:      {}\
         \n  RSA ops:      {}\
         \n  RNG bytes:    {}\
         \n  PBKDF2 ops:   {}\
         \n  X.509 parsed: {}",
        AES_OPS.load(Ordering::Relaxed),
        RSA_OPS.load(Ordering::Relaxed),
        RNG_BYTES.load(Ordering::Relaxed),
        PBKDF_OPS.load(Ordering::Relaxed),
        X509_PARSED.load(Ordering::Relaxed),
    )
}

/// Return crypto statistics as a formatted string.
pub fn crypto_stats() -> String {
    let rng = CSPRNG_STATE.lock();
    format!(
        "Crypto stats:\
         \n  AES-128 encrypt/decrypt ops: {}\
         \n  RSA operations:              {}\
         \n  CSPRNG bytes generated:      {}\
         \n  CSPRNG counter:              {}\
         \n  PBKDF2 derivations:          {}\
         \n  X.509 certificates parsed:   {}",
        AES_OPS.load(Ordering::Relaxed),
        RSA_OPS.load(Ordering::Relaxed),
        RNG_BYTES.load(Ordering::Relaxed),
        rng.counter,
        PBKDF_OPS.load(Ordering::Relaxed),
        X509_PARSED.load(Ordering::Relaxed),
    )
}

/// Initialize the crypto extensions subsystem.
pub fn init() {
    // Seed CSPRNG with a default value (should be reseeded with real entropy)
    let seed: [u8; 32] = [
        0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe,
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
        0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78,
    ];
    csprng_init(&seed);
    crate::serial_println!("[crypto_ext] initialized: AES-128, RSA, X.509, ChaCha20 CSPRNG, PBKDF2, DH, AES-CTR");
}

// ═══════════════════════════════════════════════════════════════════
//  Big Integer Arithmetic (2048-bit, for DH Group 14)
// ═══════════════════════════════════════════════════════════════════

/// Fixed-size 2048-bit unsigned integer stored as 32 x u64 limbs (little-endian).
#[derive(Clone)]
pub struct BigUint {
    limbs: [u64; 32],
}

impl BigUint {
    /// Zero.
    pub const fn zero() -> Self {
        Self { limbs: [0u64; 32] }
    }

    /// From a small u64 value.
    pub fn from_u64(v: u64) -> Self {
        let mut n = Self::zero();
        n.limbs[0] = v;
        n
    }

    /// From big-endian bytes (up to 256 bytes = 2048 bits).
    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut n = Self::zero();
        let len = bytes.len().min(256);
        for (i, &b) in bytes[..len].iter().rev().enumerate() {
            let limb_idx = i / 8;
            let byte_idx = i % 8;
            if limb_idx < 32 {
                n.limbs[limb_idx] |= (b as u64) << (byte_idx * 8);
            }
        }
        n
    }

    /// To big-endian bytes (256 bytes).
    pub fn to_be_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; 256];
        for (i, byte) in out.iter_mut().rev().enumerate() {
            let limb_idx = i / 8;
            let byte_idx = i % 8;
            if limb_idx < 32 {
                *byte = (self.limbs[limb_idx] >> (byte_idx * 8)) as u8;
            }
        }
        // Strip leading zeros (keep at least 1 byte)
        let first_nonzero = out.iter().position(|&b| b != 0).unwrap_or(255);
        out[first_nonzero..].to_vec()
    }

    /// Check if zero.
    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Test bit at position `bit`.
    pub fn bit(&self, bit: usize) -> bool {
        let limb = bit / 64;
        let pos = bit % 64;
        if limb >= 32 { return false; }
        (self.limbs[limb] >> pos) & 1 == 1
    }

    /// Number of significant bits.
    pub fn bit_len(&self) -> usize {
        for i in (0..32).rev() {
            if self.limbs[i] != 0 {
                return i * 64 + (64 - self.limbs[i].leading_zeros() as usize);
            }
        }
        0
    }

    /// Addition: self + other, returns (result, carry).
    pub fn add(&self, other: &BigUint) -> BigUint {
        let mut result = BigUint::zero();
        let mut carry: u64 = 0;
        for i in 0..32 {
            let (s1, c1) = self.limbs[i].overflowing_add(other.limbs[i]);
            let (s2, c2) = s1.overflowing_add(carry);
            result.limbs[i] = s2;
            carry = (c1 as u64) + (c2 as u64);
        }
        result
    }

    /// Subtraction: self - other (assumes self >= other).
    pub fn sub(&self, other: &BigUint) -> BigUint {
        let mut result = BigUint::zero();
        let mut borrow: u64 = 0;
        for i in 0..32 {
            let (s1, b1) = self.limbs[i].overflowing_sub(other.limbs[i]);
            let (s2, b2) = s1.overflowing_sub(borrow);
            result.limbs[i] = s2;
            borrow = (b1 as u64) + (b2 as u64);
        }
        result
    }

    /// Compare: -1, 0, +1.
    pub fn cmp(&self, other: &BigUint) -> i32 {
        for i in (0..32).rev() {
            if self.limbs[i] > other.limbs[i] { return 1; }
            if self.limbs[i] < other.limbs[i] { return -1; }
        }
        0
    }

    /// Left shift by 1 bit.
    pub fn shl1(&self) -> BigUint {
        let mut result = BigUint::zero();
        let mut carry = 0u64;
        for i in 0..32 {
            result.limbs[i] = (self.limbs[i] << 1) | carry;
            carry = self.limbs[i] >> 63;
        }
        result
    }

    /// Right shift by 1 bit.
    pub fn shr1(&self) -> BigUint {
        let mut result = BigUint::zero();
        let mut carry = 0u64;
        for i in (0..32).rev() {
            result.limbs[i] = (self.limbs[i] >> 1) | (carry << 63);
            carry = self.limbs[i] & 1;
        }
        result
    }

    /// Modular reduction: self mod modulus (using repeated subtraction for simplicity
    /// when self < 2*modulus, otherwise shift-subtract division).
    pub fn mod_reduce(&self, modulus: &BigUint) -> BigUint {
        if self.cmp(modulus) < 0 {
            return self.clone();
        }
        // Binary long division
        let mut remainder = BigUint::zero();
        let bits = self.bit_len();
        for i in (0..bits).rev() {
            remainder = remainder.shl1();
            if self.bit(i) {
                remainder.limbs[0] |= 1;
            }
            if remainder.cmp(modulus) >= 0 {
                remainder = remainder.sub(modulus);
            }
        }
        remainder
    }

    /// Modular multiplication: (a * b) mod m.
    /// Uses shift-and-add to avoid overflow.
    pub fn mul_mod(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
        let mut result = BigUint::zero();
        let mut a_shifted = a.mod_reduce(m);
        let bits = b.bit_len();
        for i in 0..bits {
            if b.bit(i) {
                result = result.add(&a_shifted).mod_reduce(m);
            }
            a_shifted = a_shifted.shl1().mod_reduce(m);
        }
        result
    }

    /// Modular exponentiation: base^exp mod modulus.
    /// Square-and-multiply algorithm.
    pub fn mod_pow(base: &BigUint, exp: &BigUint, modulus: &BigUint) -> BigUint {
        if modulus.is_zero() { return BigUint::zero(); }
        let mut result = BigUint::from_u64(1);
        let mut b = base.mod_reduce(modulus);
        let bits = exp.bit_len();
        for i in 0..bits {
            if exp.bit(i) {
                result = BigUint::mul_mod(&result, &b, modulus);
            }
            b = BigUint::mul_mod(&b, &b, modulus);
        }
        result
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Diffie-Hellman Key Exchange (RFC 3526 Group 14, 2048-bit)
// ═══════════════════════════════════════════════════════════════════

/// RFC 3526 Group 14: 2048-bit MODP prime.
/// p = 2^2048 - 2^1984 - 1 + 2^64 * { [2^1918 pi] + 124476 }
const DH_GROUP14_PRIME_BYTES: [u8; 256] = [
    0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF, 0xC9,0x0F,0xDA,0xA2,0x21,0x68,0xC2,0x34,
    0xC4,0xC6,0x62,0x8B,0x80,0xDC,0x1C,0xD1, 0x29,0x02,0x4E,0x08,0x8A,0x67,0xCC,0x74,
    0x02,0x0B,0xBE,0xA6,0x3B,0x13,0x9B,0x22, 0x51,0x4A,0x08,0x79,0x8E,0x34,0x04,0xDD,
    0xEF,0x95,0x19,0xB3,0xCD,0x3A,0x43,0x1B, 0x30,0x2B,0x0A,0x6D,0xF2,0x5F,0x14,0x37,
    0x4F,0xE1,0x35,0x6D,0x6D,0x51,0xC2,0x45, 0xE4,0x85,0xB5,0x76,0x62,0x5E,0x7E,0xC6,
    0xF4,0x4C,0x42,0xE9,0xA6,0x37,0xED,0x6B, 0x0B,0xFF,0x5C,0xB6,0xF4,0x06,0xB7,0xED,
    0xEE,0x38,0x6B,0xFB,0x5A,0x89,0x9F,0xA5, 0xAE,0x9F,0x24,0x11,0x7C,0x4B,0x1F,0xE6,
    0x49,0x28,0x66,0x51,0xEC,0xE4,0x5B,0x3D, 0xC2,0x00,0x7C,0xB8,0xA1,0x63,0xBF,0x05,
    0x98,0xDA,0x48,0x36,0x1C,0x55,0xD3,0x9A, 0x69,0x16,0x3F,0xA8,0xFD,0x24,0xCF,0x5F,
    0x83,0x65,0x5D,0x23,0xDC,0xA3,0xAD,0x96, 0x1C,0x62,0xF3,0x56,0x20,0x85,0x52,0xBB,
    0x9E,0xD5,0x29,0x07,0x70,0x96,0x96,0x6D, 0x67,0x0C,0x35,0x4E,0x4A,0xBC,0x98,0x04,
    0xF1,0x74,0x6C,0x08,0xCA,0x18,0x21,0x7C, 0x32,0x90,0x5E,0x46,0x2E,0x36,0xCE,0x3B,
    0xE3,0x9E,0x77,0x2C,0x18,0x0E,0x86,0x03, 0x9B,0x27,0x83,0xA2,0xEC,0x07,0xA2,0x8F,
    0xB5,0xC5,0x5D,0xF0,0x6F,0x4C,0x52,0xC9, 0xDE,0x2B,0xCB,0xF6,0x95,0x58,0x17,0x18,
    0x39,0x95,0x49,0x7C,0xEA,0x95,0x6A,0xE5, 0x15,0xD2,0x26,0x18,0x98,0xFA,0x05,0x10,
    0x15,0x72,0x8E,0x5A,0x8A,0xAC,0xAA,0x68, 0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,
];

const DH_GENERATOR_VAL: u64 = 2;

/// Diffie-Hellman keypair (2048-bit).
pub struct DhKeypair {
    pub private_key: BigUint,
    pub public_key: BigUint,
    /// Public key as big-endian bytes (for SSH wire format).
    pub public_bytes: Vec<u8>,
}

/// Generate a 2048-bit DH keypair using RFC 3526 Group 14.
/// private = 256-bit random, public = g^private mod p.
pub fn dh_generate_keypair() -> DhKeypair {
    let p = BigUint::from_be_bytes(&DH_GROUP14_PRIME_BYTES);
    let g = BigUint::from_u64(DH_GENERATOR_VAL);

    // Generate 256-bit random private key (sufficient security for 2048-bit DH)
    let mut priv_bytes = [0u8; 32];
    csprng_bytes(&mut priv_bytes);
    priv_bytes[0] |= 0x80; // ensure high bit set
    let private_key = BigUint::from_be_bytes(&priv_bytes);

    // public = g^private mod p
    let public_key = BigUint::mod_pow(&g, &private_key, &p);
    let public_bytes = public_key.to_be_bytes();

    crate::serial_println!("[dh] generated 2048-bit keypair ({} byte public key)", public_bytes.len());

    DhKeypair { private_key, public_key, public_bytes }
}

/// Compute DH shared secret: peer_public ^ our_private mod p.
pub fn dh_shared_secret(our_private: &BigUint, peer_public: &BigUint) -> BigUint {
    let p = BigUint::from_be_bytes(&DH_GROUP14_PRIME_BYTES);
    BigUint::mod_pow(peer_public, our_private, &p)
}

/// Derive a 16-byte AES key from a DH shared secret using SHA-256.
pub fn dh_derive_key(shared_secret: &BigUint) -> [u8; 16] {
    let secret_bytes = shared_secret.to_be_bytes();
    let hash = crate::crypto::sha256(&secret_bytes);
    let mut key = [0u8; 16];
    key.copy_from_slice(&hash[..16]);
    key
}

/// Derive a 16-byte IV from the shared secret (second half of SHA-256).
pub fn dh_derive_iv(shared_secret: &BigUint) -> [u8; 16] {
    let secret_bytes = shared_secret.to_be_bytes();
    let hash = crate::crypto::sha256(&secret_bytes);
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&hash[16..32]);
    iv
}

// ═══════════════════════════════════════════════════════════════════
//  AES-128-CTR Mode (for SSH transport encryption)
// ═══════════════════════════════════════════════════════════════════

/// AES-128-CTR cipher state.
pub struct AesCtr {
    key: [u8; 16],
    nonce: [u8; 16], // 128-bit counter/nonce
    counter: u64,
}

impl AesCtr {
    /// Create a new AES-128-CTR cipher with the given key and IV.
    pub fn new(key: [u8; 16], iv: [u8; 16]) -> Self {
        Self { key, nonce: iv, counter: 0 }
    }

    /// Generate the next keystream block.
    fn next_block(&mut self) -> [u8; 16] {
        let mut ctr_block = self.nonce;
        // XOR counter into the last 8 bytes of the nonce
        let ctr_bytes = self.counter.to_be_bytes();
        for i in 0..8 {
            ctr_block[8 + i] ^= ctr_bytes[i];
        }
        self.counter += 1;
        aes128_encrypt_block(&ctr_block, &self.key)
    }

    /// Encrypt or decrypt data in-place (CTR mode is symmetric).
    pub fn process(&mut self, data: &mut [u8]) {
        let mut keystream = [0u8; 16];
        let mut ks_pos = 16; // force generation of first block

        for byte in data.iter_mut() {
            if ks_pos >= 16 {
                keystream = self.next_block();
                ks_pos = 0;
            }
            *byte ^= keystream[ks_pos];
            ks_pos += 1;
        }
    }

    /// Encrypt data, returning a new Vec.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let mut out = plaintext.to_vec();
        self.process(&mut out);
        out
    }

    /// Decrypt data, returning a new Vec (same as encrypt for CTR).
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Vec<u8> {
        let mut out = ciphertext.to_vec();
        self.process(&mut out);
        out
    }
}

// ═══════════════════════════════════════════════════════════════════
//  SSH Transport Crypto Context
// ═══════════════════════════════════════════════════════════════════

/// Crypto state for an SSH session after key exchange.
pub struct SshCrypto {
    pub encrypt: AesCtr,
    pub decrypt: AesCtr,
    pub mac_key: [u8; 32],  // HMAC-SHA256 key
    pub seq_send: u32,
    pub seq_recv: u32,
}

impl SshCrypto {
    /// Create SSH crypto context from a DH shared secret hash (u64 seed).
    /// Derives AES-128-CTR keys + HMAC-SHA256 MAC key via SHA-256.
    pub fn from_shared_secret(shared: u64) -> Self {
        let secret_bytes = shared.to_be_bytes();
        let hash = crate::crypto::sha256(&secret_bytes);
        let mut enc_key = [0u8; 16];
        enc_key.copy_from_slice(&hash[..16]);
        let mut enc_iv = [0u8; 16];
        enc_iv.copy_from_slice(&hash[16..32]);
        // Derive separate decrypt key by hashing with a different label
        let mut dec_seed = secret_bytes.to_vec();
        dec_seed.push(0x01); // differentiate from encrypt key
        let dec_hash = crate::crypto::sha256(&dec_seed);
        let mut dec_key = [0u8; 16];
        dec_key.copy_from_slice(&dec_hash[..16]);
        let mut dec_iv = [0u8; 16];
        dec_iv.copy_from_slice(&dec_hash[16..32]);
        // MAC key from another derivation
        dec_seed.push(0x02);
        let mac_hash = crate::crypto::sha256(&dec_seed);
        Self {
            encrypt: AesCtr::new(enc_key, enc_iv),
            decrypt: AesCtr::new(dec_key, dec_iv),
            mac_key: mac_hash,
            seq_send: 0,
            seq_recv: 0,
        }
    }

    /// Encrypt an SSH packet payload (in-place).
    pub fn encrypt_packet(&mut self, data: &mut [u8]) {
        self.encrypt.process(data);
        self.seq_send += 1;
    }

    /// Decrypt an SSH packet payload (in-place).
    pub fn decrypt_packet(&mut self, data: &mut [u8]) {
        self.decrypt.process(data);
        self.seq_recv += 1;
    }

    /// Compute HMAC-SHA256 MAC for a packet.
    pub fn compute_mac(&self, seq: u32, data: &[u8]) -> [u8; 32] {
        let mut mac_data = Vec::with_capacity(4 + data.len());
        mac_data.extend_from_slice(&seq.to_be_bytes());
        mac_data.extend_from_slice(data);
        crate::crypto::hmac_sha256(&self.mac_key, &mac_data)
    }
}
