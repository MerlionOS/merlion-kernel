/// User-space library (ulib).
/// Provides Rust-callable syscall wrappers and a mini runtime for
/// user programs. These functions compile into the kernel but are
/// designed to be used by user-mode code running via int 0x80.
///
/// In a future phase, this would be split into a separate crate
/// compiled as a static library for standalone user binaries.

/// Raw syscall: invoke int 0x80 with up to 3 arguments.
#[inline(always)]
pub fn syscall(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inout("rax") num => ret,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
        );
    }
    ret
}

/// Write a string to the kernel console.
pub fn write(s: &str) {
    syscall(0, s.as_ptr() as u64, s.len() as u64, 0);
}

/// Exit the current process with the given code.
pub fn exit(code: u64) -> ! {
    syscall(1, code, 0, 0);
    loop {} // should never reach here
}

/// Yield the CPU to the scheduler.
pub fn yield_now() {
    syscall(2, 0, 0, 0);
}

/// Get the current process ID.
pub fn getpid() -> u64 {
    syscall(3, 0, 0, 0)
}

/// Sleep for the specified number of timer ticks.
pub fn sleep(ticks: u64) {
    syscall(4, ticks, 0, 0);
}

/// Send a byte to an IPC channel.
pub fn send(channel: u64, byte: u8) {
    syscall(5, channel, byte as u64, 0);
}

/// Receive a byte from an IPC channel (blocking).
pub fn recv(channel: u64) -> u64 {
    syscall(6, channel, 0, 0)
}

// --- Convenience functions ---

/// Write a string followed by a newline.
pub fn println(s: &str) {
    write(s);
    write("\n");
}

/// Simple number-to-string for user programs (no allocator needed).
pub fn write_num(n: u64) {
    if n == 0 {
        write("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 20;
    let mut val = n;
    while val > 0 {
        i -= 1;
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    if let Ok(s) = core::str::from_utf8(&buf[i..]) {
        write(s);
    }
}

// --- User program examples as Rust functions ---
// These can be used with process::spawn or compiled into user binaries.

/// Example user program: hello world.
pub fn program_hello() {
    println("Hello from user-space (Rust)!");
    println("This message was sent via sys_write (int 0x80).");
    exit(0);
}

/// Example user program: counter with yield.
pub fn program_counter() {
    write("Counting: ");
    for i in 1..=5 {
        write_num(i);
        write(" ");
        yield_now();
    }
    println("");
    println("Done counting.");
    exit(0);
}
