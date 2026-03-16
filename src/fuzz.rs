/// Kernel fuzzing and stress test framework for MerlionOS.
/// Generates random inputs to test kernel subsystems for robustness.
/// Uses a simple PRNG (xorshift64) for deterministic test reproduction.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Xorshift64 PRNG state.
static RNG_STATE: AtomicU64 = AtomicU64::new(0x1234_5678_9ABC_DEF0);

/// Total fuzz iterations run.
static ITERATIONS: AtomicUsize = AtomicUsize::new(0);
/// Total failures detected.
static FAILURES: AtomicUsize = AtomicUsize::new(0);
/// Total passes.
static PASSES: AtomicUsize = AtomicUsize::new(0);

/// Fuzz test result.
#[derive(Debug, Clone)]
pub struct FuzzResult {
    pub test_name: String,
    pub iterations: usize,
    pub passes: usize,
    pub failures: usize,
    pub failure_details: Vec<String>,
}

/// Seed the PRNG for reproducible tests.
pub fn seed(s: u64) {
    let s = if s == 0 { 1 } else { s };
    RNG_STATE.store(s, Ordering::SeqCst);
}

/// Generate a pseudo-random u64.
pub fn rand_u64() -> u64 {
    let mut s = RNG_STATE.load(Ordering::Relaxed);
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    RNG_STATE.store(s, Ordering::Relaxed);
    s
}

/// Generate a random u64 in range [0, max).
pub fn rand_range(max: u64) -> u64 {
    if max == 0 { return 0; }
    rand_u64() % max
}

/// Generate a random byte.
pub fn rand_byte() -> u8 {
    rand_u64() as u8
}

/// Generate a random string of given length (ASCII printable).
pub fn rand_string(len: usize) -> String {
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        let ch = (rand_range(95) as u8 + 32) as char; // ASCII 32-126
        s.push(ch);
    }
    s
}

/// Generate a random path-like string.
pub fn rand_path() -> String {
    let depth = rand_range(4) as usize + 1;
    let mut path = String::from("/");
    for i in 0..depth {
        if i > 0 { path.push('/'); }
        let seg_len = rand_range(8) as usize + 1;
        for _ in 0..seg_len {
            let ch = (rand_range(26) as u8 + b'a') as char;
            path.push(ch);
        }
    }
    path
}

/// Fuzz the VFS subsystem with random read/write/create/delete operations.
pub fn fuzz_vfs(iterations: usize) -> FuzzResult {
    let mut result = FuzzResult {
        test_name: "VFS fuzz".to_owned(),
        iterations, passes: 0, failures: 0,
        failure_details: Vec::new(),
    };

    for _ in 0..iterations {
        let op = rand_range(5);
        match op {
            0 => {
                // Random cat
                let path = rand_path();
                let _ = crate::vfs::cat(&path);
                result.passes += 1;
            }
            1 => {
                // Write random data to /tmp
                let name = rand_string(rand_range(8) as usize + 1);
                let path = format!("/tmp/{}", name);
                let data = rand_string(rand_range(256) as usize);
                match crate::vfs::write(&path, &data) {
                    Ok(()) => result.passes += 1,
                    Err(_) => result.passes += 1, // errors are expected, not failures
                }
            }
            2 => {
                // List random directory
                let path = rand_path();
                let _ = crate::vfs::ls(&path);
                result.passes += 1;
            }
            3 => {
                // Remove random file
                let path = rand_path();
                let _ = crate::vfs::rm(&path);
                result.passes += 1;
            }
            4 => {
                // Check file exists
                let path = rand_path();
                let _ = crate::vfs::exists(&path);
                result.passes += 1;
            }
            _ => {}
        }
        ITERATIONS.fetch_add(1, Ordering::Relaxed);
    }

    result
}

/// Fuzz the security subsystem with random permission checks and user operations.
pub fn fuzz_security(iterations: usize) -> FuzzResult {
    let mut result = FuzzResult {
        test_name: "Security fuzz".to_owned(),
        iterations, passes: 0, failures: 0,
        failure_details: Vec::new(),
    };

    for _ in 0..iterations {
        let op = rand_range(6);
        match op {
            0 => {
                // Random permission check
                let path = rand_path();
                let _ = crate::security::can_read(&path);
                let _ = crate::security::can_write(&path);
                result.passes += 1;
            }
            1 => {
                // Random chmod
                let path = rand_path();
                let mode = rand_range(0o777) as u16;
                let _ = crate::security::chmod(&path, mode);
                result.passes += 1;
            }
            2 => {
                // Random authentication
                let user = rand_string(rand_range(8) as usize + 1);
                let hash = rand_u64();
                let _ = crate::security::authenticate(&user, hash);
                result.passes += 1;
            }
            3 => {
                // Get permission for random path
                let path = rand_path();
                let _ = crate::security::get_permission(&path);
                result.passes += 1;
            }
            4 => {
                // User lookup
                let _ = crate::security::whoami();
                let _ = crate::security::current_uid();
                result.passes += 1;
            }
            5 => {
                // ID info
                let _ = crate::security::id_info(None);
                result.passes += 1;
            }
            _ => {}
        }
        ITERATIONS.fetch_add(1, Ordering::Relaxed);
    }

    result
}

/// Fuzz the IPC subsystem with random send/recv on random channels.
pub fn fuzz_ipc(iterations: usize) -> FuzzResult {
    let mut result = FuzzResult {
        test_name: "IPC fuzz".to_owned(),
        iterations, passes: 0, failures: 0,
        failure_details: Vec::new(),
    };

    for _ in 0..iterations {
        let op = rand_range(2);
        match op {
            0 => {
                let ch = rand_range(4) as usize;
                let byte = rand_byte();
                let _ = crate::ipc::send(ch, byte);
                result.passes += 1;
            }
            1 => {
                let ch = rand_range(4) as usize;
                let _ = crate::ipc::recv(ch);
                result.passes += 1;
            }
            _ => {}
        }
        ITERATIONS.fetch_add(1, Ordering::Relaxed);
    }

    result
}

/// Fuzz string/parsing utilities with random inputs.
pub fn fuzz_parsers(iterations: usize) -> FuzzResult {
    let mut result = FuzzResult {
        test_name: "Parser fuzz".to_owned(),
        iterations, passes: 0, failures: 0,
        failure_details: Vec::new(),
    };

    for _ in 0..iterations {
        let op = rand_range(4);
        match op {
            0 => {
                // JSON parse
                let input = rand_string(rand_range(64) as usize + 1);
                let _ = crate::json::parse(&input);
                result.passes += 1;
            }
            1 => {
                // TOML parse
                let input = rand_string(rand_range(64) as usize + 1);
                let _ = crate::toml::parse(&input);
                result.passes += 1;
            }
            2 => {
                // Regex compile
                let pattern = rand_string(rand_range(16) as usize + 1);
                let _ = crate::regex::Regex::compile(&pattern);
                result.passes += 1;
            }
            3 => {
                // Glob match
                let pattern = rand_string(rand_range(16) as usize + 1);
                let input = rand_string(rand_range(32) as usize + 1);
                let _ = crate::glob::glob_match(&pattern, &input);
                result.passes += 1;
            }
            _ => {}
        }
        ITERATIONS.fetch_add(1, Ordering::Relaxed);
    }

    result
}

/// Run all fuzz tests with the given iteration count per test.
pub fn fuzz_all(iterations_per_test: usize) -> String {
    let tests = [
        fuzz_vfs(iterations_per_test),
        fuzz_security(iterations_per_test),
        fuzz_ipc(iterations_per_test),
        fuzz_parsers(iterations_per_test),
    ];

    let mut out = String::from("=== MerlionOS Fuzz Test Results ===\n\n");

    let mut total_pass = 0usize;
    let mut total_fail = 0usize;

    for test in &tests {
        let status = if test.failures == 0 { "PASS" } else { "FAIL" };
        out.push_str(&format!(
            "[{}] {} — {} iterations, {} passed, {} failed\n",
            status, test.test_name, test.iterations, test.passes, test.failures
        ));
        for detail in &test.failure_details {
            out.push_str(&format!("  ! {}\n", detail));
        }
        total_pass += test.passes;
        total_fail += test.failures;
    }

    out.push_str(&format!(
        "\nTotal: {} passed, {} failed ({} iterations)\n",
        total_pass, total_fail, total_pass + total_fail
    ));

    PASSES.store(total_pass, Ordering::Relaxed);
    FAILURES.store(total_fail, Ordering::Relaxed);

    out
}

/// Get overall fuzz statistics.
pub fn fuzz_stats() -> String {
    format!(
        "Fuzz stats: {} total iterations, {} passes, {} failures",
        ITERATIONS.load(Ordering::Relaxed),
        PASSES.load(Ordering::Relaxed),
        FAILURES.load(Ordering::Relaxed),
    )
}

/// Format a single fuzz result.
pub fn format_result(result: &FuzzResult) -> String {
    let status = if result.failures == 0 { "PASS" } else { "FAIL" };
    let mut out = format!(
        "[{}] {} — {}/{} passed\n",
        status, result.test_name, result.passes, result.iterations
    );
    for detail in &result.failure_details {
        out.push_str(&format!("  {}\n", detail));
    }
    out
}
