/// Built-in unit test framework for MerlionOS.
///
/// Provides a structured test runner with assertion macros, test suites,
/// and formatted reports. Since the kernel runs in `no_std` without access
/// to `cargo test`, this module gives us a self-contained way to verify
/// subsystem correctness at runtime.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use alloc::borrow::ToOwned;
use crate::{println, print, serial_println};

// ---- assertion macros ----------------------------------------------------

/// Assert that a boolean condition is true.
#[macro_export]
macro_rules! kassert {
    ($cond:expr) => {
        if !($cond) {
            return Err(alloc::format!("assertion failed: `{}` at {}:{}", stringify!($cond), file!(), line!()));
        }
    };
    ($cond:expr, $($arg:tt)+) => {
        if !($cond) {
            return Err(alloc::format!("assertion failed: {} at {}:{}", alloc::format!($($arg)+), file!(), line!()));
        }
    };
}

/// Assert that two values are equal.
#[macro_export]
macro_rules! kassert_eq {
    ($left:expr, $right:expr) => {{
        let (l, r) = (&$left, &$right);
        if *l != *r {
            return Err(alloc::format!(
                "assertion `{} == {}` failed\n  left:  {:?}\n  right: {:?}\n  at {}:{}",
                stringify!($left), stringify!($right), l, r, file!(), line!()));
        }
    }};
}

/// Assert that two values are not equal.
#[macro_export]
macro_rules! kassert_ne {
    ($left:expr, $right:expr) => {{
        let (l, r) = (&$left, &$right);
        if *l == *r {
            return Err(alloc::format!(
                "assertion `{} != {}` failed — both are {:?} at {}:{}",
                stringify!($left), stringify!($right), l, file!(), line!()));
        }
    }};
}

/// Assert that a string contains a given substring.
#[macro_export]
macro_rules! kassert_str_contains {
    ($haystack:expr, $needle:expr) => {{
        let (h, n): (&str, &str) = (&$haystack, &$needle);
        if !h.contains(n) {
            return Err(alloc::format!(
                "\"{}\" does not contain \"{}\" at {}:{}", h, n, file!(), line!()));
        }
    }};
}

// ---- core types ----------------------------------------------------------

/// A single named test returning `Ok(())` on success or an error message.
pub struct TestCase {
    /// Human-readable test name.
    pub name: &'static str,
    /// Test body.
    pub test_fn: fn() -> Result<(), String>,
}

/// A named collection of related test cases.
pub struct TestSuite {
    /// Suite name (e.g. "allocator", "vfs").
    pub name: &'static str,
    /// Individual tests.
    pub cases: Vec<TestCase>,
}

impl TestSuite {
    /// Create a new empty suite.
    pub fn new(name: &'static str) -> Self { Self { name, cases: Vec::new() } }
    /// Add a test case.
    pub fn add(&mut self, name: &'static str, f: fn() -> Result<(), String>) {
        self.cases.push(TestCase { name, test_fn: f });
    }
}

/// Aggregated results from running one or more test suites.
pub struct TestReport {
    /// Tests that passed.
    pub passed: usize,
    /// Tests that failed.
    pub failed: usize,
    /// Total tests executed.
    pub total: usize,
    /// Failure details: (suite, test, message).
    pub failures: Vec<(String, String, String)>,
}

impl TestReport {
    fn new() -> Self { Self { passed: 0, failed: 0, total: 0, failures: Vec::new() } }

    /// Display a formatted summary to the console and serial port.
    pub fn display(&self) {
        println!();
        println!("========================================");
        println!("  Test Report: {} passed, {} failed, {} total", self.passed, self.failed, self.total);
        println!("========================================");
        if !self.failures.is_empty() {
            println!("\nFailures:");
            for (suite, test, msg) in &self.failures {
                println!("  [{}::{}] {}", suite, test, msg);
            }
        }
        println!();
        if self.failed == 0 {
            println!("\x1b[32mResult: ALL TESTS PASSED\x1b[0m");
        } else {
            println!("\x1b[31mResult: {} FAILURE(S)\x1b[0m", self.failed);
        }
        serial_println!("unittest: {}/{} passed", self.passed, self.total);
    }
}

// ---- runner --------------------------------------------------------------

/// Run a single test suite, merging results into `report`.
pub fn run_suite(suite: &TestSuite, report: &mut TestReport) {
    println!("\n--- suite: {} ({} tests) ---", suite.name, suite.cases.len());
    serial_println!("unittest suite: {}", suite.name);
    for case in &suite.cases {
        print!("  {}::{} ... ", suite.name, case.name);
        report.total += 1;
        match (case.test_fn)() {
            Ok(()) => {
                report.passed += 1;
                println!("\x1b[32mok\x1b[0m");
                serial_println!("  {} ... ok", case.name);
            }
            Err(msg) => {
                report.failed += 1;
                println!("\x1b[31mFAIL\x1b[0m");
                serial_println!("  {} ... FAIL: {}", case.name, msg);
                report.failures.push((suite.name.to_owned(), case.name.to_owned(), msg));
            }
        }
    }
}

// ---- built-in suites -----------------------------------------------------

/// Run every built-in test suite and return the combined report.
pub fn run_all() -> TestReport {
    let mut report = TestReport::new();
    run_suite(&suite_allocator(), &mut report);
    run_suite(&suite_vfs(), &mut report);
    run_suite(&suite_ipc(), &mut report);
    run_suite(&suite_calc(), &mut report);
    run_suite(&suite_regex(), &mut report);
    run_suite(&suite_json(), &mut report);
    report.display();
    report
}

/// Allocator tests: alloc and free via Vec, Box, String.
fn suite_allocator() -> TestSuite {
    let mut s = TestSuite::new("allocator");
    s.add("vec_alloc_free", || {
        let mut v: Vec<u64> = Vec::new();
        for i in 0..128 { v.push(i); }
        kassert_eq!(v.len(), 128);
        kassert_eq!(v[127], 127);
        drop(v);
        let stats = crate::allocator::stats();
        kassert!(stats.free > 0, "heap should have free space after drop");
        Ok(())
    });
    s.add("box_alloc_free", || {
        let b = alloc::boxed::Box::new(0xDEAD_BEEFu64);
        kassert_eq!(*b, 0xDEAD_BEEF);
        drop(b);
        Ok(())
    });
    s.add("string_alloc", || {
        let s = String::from("MerlionOS unittest");
        kassert_eq!(s.len(), 18);
        kassert!(s.starts_with("Merlion"));
        Ok(())
    });
    s
}

/// VFS tests: write, read, delete.
fn suite_vfs() -> TestSuite {
    let mut s = TestSuite::new("vfs");
    s.add("write_read", || {
        crate::vfs::write("/tmp/_ut_wr", "hello vfs").map_err(|e| format!("write: {}", e))?;
        let content = crate::vfs::cat("/tmp/_ut_wr").map_err(|e| format!("cat: {}", e))?;
        kassert_eq!(content.as_str(), "hello vfs");
        crate::vfs::rm("/tmp/_ut_wr").map_err(|e| format!("rm: {}", e))?;
        Ok(())
    });
    s.add("delete_missing", || {
        let res = crate::vfs::rm("/tmp/_ut_nonexistent");
        kassert!(res.is_err(), "deleting a missing file should fail");
        Ok(())
    });
    s.add("ls_root", || {
        let entries = crate::vfs::ls("/").map_err(|e| format!("ls: {}", e))?;
        kassert!(entries.len() >= 2, "root should have at least dev and proc");
        Ok(())
    });
    s
}

/// IPC tests: send and receive through channels.
fn suite_ipc() -> TestSuite {
    let mut s = TestSuite::new("ipc");
    s.add("send_recv_bytes", || {
        let ch = crate::ipc::create().ok_or_else(|| "failed to create channel".to_owned())?;
        kassert!(crate::ipc::send(ch, b'X'));
        kassert!(crate::ipc::send(ch, b'Y'));
        kassert_eq!(crate::ipc::recv(ch), Some(b'X'));
        kassert_eq!(crate::ipc::recv(ch), Some(b'Y'));
        kassert_eq!(crate::ipc::recv(ch), None);
        crate::ipc::destroy(ch);
        Ok(())
    });
    s.add("send_recv_str", || {
        let ch = crate::ipc::create().ok_or_else(|| "failed to create channel".to_owned())?;
        let n = crate::ipc::send_str(ch, "test");
        kassert_eq!(n, 4);
        let got = crate::ipc::recv_all(ch);
        kassert_eq!(got.as_str(), "test");
        crate::ipc::destroy(ch);
        Ok(())
    });
    s
}

/// Calc tests: evaluate arithmetic expressions.
fn suite_calc() -> TestSuite {
    let mut s = TestSuite::new("calc");
    s.add("basic_add", || {
        kassert_eq!(crate::calc::eval("2 + 3").map_err(|e| e.to_owned())?, 5);
        Ok(())
    });
    s.add("precedence", || {
        kassert_eq!(crate::calc::eval("2 + 3 * 4").map_err(|e| e.to_owned())?, 14);
        Ok(())
    });
    s.add("parentheses", || {
        kassert_eq!(crate::calc::eval("(2 + 3) * 4").map_err(|e| e.to_owned())?, 20);
        Ok(())
    });
    s.add("division_modulo", || {
        kassert_eq!(crate::calc::eval("10 / 3").map_err(|e| e.to_owned())?, 3);
        kassert_eq!(crate::calc::eval("10 % 3").map_err(|e| e.to_owned())?, 1);
        Ok(())
    });
    s
}

/// Regex tests: compile patterns and match strings.
fn suite_regex() -> TestSuite {
    let mut s = TestSuite::new("regex");
    s.add("literal_match", || {
        let re = crate::regex::Regex::compile("hello").map_err(|e| format!("{:?}", e))?;
        kassert!(re.is_match("hello world"));
        kassert!(!re.is_match("goodbye"));
        Ok(())
    });
    s.add("dot_star", || {
        let re = crate::regex::Regex::compile("h.*d").map_err(|e| format!("{:?}", e))?;
        kassert!(re.is_match("helloworld"));
        kassert!(!re.is_match("xyz"));
        Ok(())
    });
    s.add("char_class", || {
        let re = crate::regex::Regex::compile("[aeiou]+").map_err(|e| format!("{:?}", e))?;
        kassert!(re.is_match("hello"));
        kassert!(!re.is_match("rhythm"));
        Ok(())
    });
    s.add("anchors", || {
        let re = crate::regex::Regex::compile("^abc$").map_err(|e| format!("{:?}", e))?;
        kassert!(re.is_match("abc"));
        kassert!(!re.is_match("xabc"));
        kassert!(!re.is_match("abcx"));
        Ok(())
    });
    s
}

/// JSON tests: parse and stringify values.
fn suite_json() -> TestSuite {
    let mut s = TestSuite::new("json");
    s.add("parse_object", || {
        let val = crate::json::parse(r#"{"key": "value", "n": 42}"#).map_err(|e| e.to_owned())?;
        kassert_eq!(crate::json::get(&val, "key"), Some(&crate::json::JsonValue::String("value".to_owned())));
        kassert_eq!(crate::json::get(&val, "n"), Some(&crate::json::JsonValue::Number(42)));
        Ok(())
    });
    s.add("parse_array", || {
        let val = crate::json::parse("[1, 2, 3]").map_err(|e| e.to_owned())?;
        if let crate::json::JsonValue::Array(arr) = &val {
            kassert_eq!(arr.len(), 3);
            kassert_eq!(arr[0], crate::json::JsonValue::Number(1));
        } else {
            return Err("expected array".to_owned());
        }
        Ok(())
    });
    s.add("stringify_roundtrip", || {
        let input = r#"{"a":1,"b":"hello","c":true}"#;
        let val = crate::json::parse(input).map_err(|e| e.to_owned())?;
        let output = crate::json::stringify(&val);
        let val2 = crate::json::parse(&output).map_err(|e| e.to_owned())?;
        kassert_eq!(val, val2);
        Ok(())
    });
    s.add("parse_null_bool", || {
        kassert_eq!(crate::json::parse("null").map_err(|e| e.to_owned())?, crate::json::JsonValue::Null);
        kassert_eq!(crate::json::parse("true").map_err(|e| e.to_owned())?, crate::json::JsonValue::Bool(true));
        kassert_eq!(crate::json::parse("false").map_err(|e| e.to_owned())?, crate::json::JsonValue::Bool(false));
        Ok(())
    });
    s
}
