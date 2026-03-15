/// Built-in kernel self-tests.
/// Run via the `test` shell command. Tests run inline in the kernel.

use crate::{serial_println, println, print, allocator, timer, rtc, ipc, vfs, task, memory};
use alloc::vec::Vec;
use alloc::string::String;

struct TestResult {
    #[allow(dead_code)]
    name: &'static str,
    passed: bool,
}

/// Run all kernel self-tests and report results.
pub fn run_all() -> (usize, usize) {
    let mut results = Vec::new();

    run(&mut results, "trivial assertion", || {
        assert_eq!(1 + 1, 2);
    });

    run(&mut results, "heap alloc vec", || {
        let mut v: Vec<u64> = Vec::new();
        for i in 0..100 {
            v.push(i);
        }
        assert_eq!(v.len(), 100);
        assert_eq!(v[99], 99);
    });

    run(&mut results, "heap alloc string", || {
        let s = String::from("MerlionOS");
        assert_eq!(s.len(), 9);
        assert!(s.contains("Merlion"));
    });

    run(&mut results, "heap alloc box", || {
        let b = alloc::boxed::Box::new(42u64);
        assert_eq!(*b, 42);
    });

    run(&mut results, "timer ticking", || {
        let t1 = timer::ticks();
        // Busy wait briefly
        for _ in 0..10000 { core::hint::spin_loop(); }
        let t2 = timer::ticks();
        assert!(t2 >= t1, "timer should not go backwards");
    });

    run(&mut results, "rtc date valid", || {
        let dt = rtc::read();
        assert!(dt.year >= 2024 && dt.year <= 2100);
        assert!(dt.month >= 1 && dt.month <= 12);
        assert!(dt.day >= 1 && dt.day <= 31);
        assert!(dt.hour <= 23);
        assert!(dt.minute <= 59);
        assert!(dt.second <= 59);
    });

    run(&mut results, "ipc channel", || {
        let ch = ipc::create().expect("create channel");
        assert!(ipc::send(ch, b'A'));
        assert!(ipc::send(ch, b'B'));
        assert_eq!(ipc::recv(ch), Some(b'A'));
        assert_eq!(ipc::recv(ch), Some(b'B'));
        assert_eq!(ipc::recv(ch), None);
        ipc::destroy(ch);
    });

    run(&mut results, "ipc send_str", || {
        let ch = ipc::create().expect("create channel");
        let sent = ipc::send_str(ch, "hello");
        assert_eq!(sent, 5);
        let received = ipc::recv_all(ch);
        assert_eq!(received, "hello");
        ipc::destroy(ch);
    });

    run(&mut results, "vfs ls root", || {
        let entries = vfs::ls("/").expect("ls /");
        assert!(entries.len() >= 3); // dev, proc, tmp
    });

    run(&mut results, "vfs write read", || {
        vfs::write("/tmp/test_file", "test data").expect("write");
        let content = vfs::cat("/tmp/test_file").expect("cat");
        assert_eq!(content, "test data");
        vfs::rm("/tmp/test_file").expect("rm");
    });

    run(&mut results, "vfs proc uptime", || {
        let content = vfs::cat("/proc/uptime").expect("cat uptime");
        assert!(content.contains("ticks"));
    });

    run(&mut results, "vfs dev null", || {
        vfs::write("/dev/null", "discard this").expect("write null");
        let content = vfs::cat("/dev/null").expect("cat null");
        assert!(content.is_empty());
    });

    run(&mut results, "memory stats", || {
        let stats = memory::stats();
        assert!(stats.total_usable_bytes > 0);
        assert!(stats.allocated_frames > 0);
    });

    run(&mut results, "allocator stats", || {
        let stats = allocator::stats();
        assert!(stats.total > 0);
        assert!(stats.used + stats.free <= stats.total + 64); // small alignment slack
    });

    run(&mut results, "task list", || {
        let tasks = task::list();
        assert!(!tasks.is_empty());
        // Kernel task should always be present
        assert!(tasks.iter().any(|t| t.name == "kernel"));
    });

    // Summary
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();

    println!();
    if passed == total {
        println!("\x1b[32mAll {} tests passed!\x1b[0m", total);
    } else {
        println!("\x1b[31m{}/{} tests passed\x1b[0m", passed, total);
    }

    (passed, total)
}

fn run(results: &mut Vec<TestResult>, name: &'static str, test: impl FnOnce()) {
    use core::sync::atomic::{AtomicBool, Ordering};

    static TEST_PANICKED: AtomicBool = AtomicBool::new(false);
    TEST_PANICKED.store(false, Ordering::SeqCst);

    // We can't catch panics in no_std, so just run and hope for the best.
    // If a test panics, the kernel panic handler will fire.
    print!("  {}... ", name);
    test();
    let passed = !TEST_PANICKED.load(Ordering::SeqCst);

    if passed {
        println!("\x1b[32mok\x1b[0m");
        serial_println!("  {}... ok", name);
    } else {
        println!("\x1b[31mFAIL\x1b[0m");
        serial_println!("  {}... FAIL", name);
    }

    results.push(TestResult { name, passed });
}
