/// Syscall dispatch (int 0x80).
/// ABI: rax = syscall number, rdi = arg1, rsi = arg2, rdx = arg3.
///
/// Syscalls:
///   0 (SYS_WRITE):     write(buf, len) — print to serial+VGA
///   1 (SYS_EXIT):      exit(code) — terminate current task
///   2 (SYS_YIELD):     yield() — yield to scheduler
///   3 (SYS_GETPID):    getpid() — return current PID
///   4 (SYS_SLEEP):     sleep(ticks) — busy-wait for N timer ticks
///   5 (SYS_SEND):      send(channel_id, byte) — send byte to IPC channel
///   6 (SYS_RECV):      recv(channel_id) — receive byte from IPC channel
///   7 (SYS_GETUID):    getuid() — return current UID
///   8 (SYS_SETUID):    setuid(uid) — set UID (requires CAP_SETUID)
///   9 (SYS_GETGID):    getgid() — return current GID
///  10 (SYS_SETGID):    setgid(gid) — set GID (requires CAP_SETGID)
///  11 (SYS_GETGROUPS): getgroups() — return group count
///  12 (SYS_CHMOD):     chmod(path_ptr, mode) — change file permissions
///  13 (SYS_CHOWN):     chown(path_ptr, uid_gid) — change ownership
///  14 (SYS_ACCESS):    access(path_ptr, mode) — check file access
///
/// File operations:
/// 100 (SYS_OPEN):     open(path_ptr, path_len, flags) → fd
/// 101 (SYS_READ):     read(fd, buf_ptr, len) → bytes_read
/// 102 (SYS_CLOSE):    close(fd) → 0
/// 103 (SYS_STAT):     stat(path_ptr, path_len, buf_ptr) → 0
/// 104 (SYS_LSEEK):    lseek(fd, offset, whence) → new_offset
/// 105 (SYS_MKDIR):    mkdir(path_ptr, path_len) → 0
/// 106 (SYS_UNLINK):   unlink(path_ptr, path_len) → 0
/// 107 (SYS_READDIR):  readdir(path_ptr, path_len, buf_ptr, buf_len) → bytes
/// 108 (SYS_CHDIR):    chdir(path_ptr, path_len) → 0
/// 109 (SYS_GETCWD):   getcwd(buf_ptr, buf_len) → len
///
/// Process operations:
/// 110 (SYS_FORK):     fork() → child_pid (0 in child)
/// 111 (SYS_EXEC):     exec(path_ptr, path_len) → noreturn
/// 112 (SYS_WAITPID):  waitpid(pid) → exit_code
/// 113 (SYS_BRK):      brk(addr) → new_brk
/// 114 (SYS_GETPPID):  getppid() → parent_pid
/// 115 (SYS_KILL):     kill(pid, signal) → 0
///
/// Memory operations:
/// 120 (SYS_MMAP):     mmap(addr, len, prot, flags) → mapped_addr
/// 121 (SYS_MUNMAP):   munmap(addr, len) → 0
/// 122 (SYS_MPROTECT): mprotect(addr, len, prot) → 0
///
/// Network operations:
/// 130 (SYS_SOCKET):   socket(domain, type, protocol) → fd
/// 131 (SYS_CONNECT):  connect(fd, addr_ptr, addr_len) → 0
/// 132 (SYS_SENDTO):   sendto(fd, buf_ptr, len) → bytes_sent
/// 133 (SYS_RECVFROM): recvfrom(fd, buf_ptr, len) → bytes_received
/// 134 (SYS_BIND):     bind(fd, addr_ptr, addr_len) → 0
/// 135 (SYS_LISTEN):   listen(fd, backlog) → 0
/// 136 (SYS_ACCEPT):   accept(fd) → new_fd
///
/// Time operations:
/// 140 (SYS_TIME):          time() → epoch_seconds
/// 141 (SYS_NANOSLEEP):     nanosleep(ms) → 0
/// 142 (SYS_CLOCK_GETTIME): clock_gettime(buf_ptr) → 0
///
/// Misc operations:
/// 150 (SYS_IOCTL):    ioctl(fd, request, arg) → result
/// 151 (SYS_PIPE):     pipe(fds_ptr) → 0 (writes [read_fd, write_fd])
/// 152 (SYS_DUP2):     dup2(oldfd, newfd) → newfd

use alloc::string::String;
use crate::{serial_println, klog_println, println, task, timer, ipc};

// Original syscalls (0-14)
const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 1;
const SYS_YIELD: u64 = 2;
const SYS_GETPID: u64 = 3;
const SYS_SLEEP: u64 = 4;
const SYS_SEND: u64 = 5;
const SYS_RECV: u64 = 6;
const SYS_GETUID: u64 = 7;
const SYS_SETUID: u64 = 8;
const SYS_GETGID: u64 = 9;
const SYS_SETGID: u64 = 10;
const SYS_GETGROUPS: u64 = 11;
const SYS_CHMOD: u64 = 12;
const SYS_CHOWN: u64 = 13;
const SYS_ACCESS: u64 = 14;

// File operations (100-109)
const SYS_OPEN: u64 = 100;
const SYS_READ: u64 = 101;
const SYS_CLOSE: u64 = 102;
const SYS_STAT: u64 = 103;
const SYS_LSEEK: u64 = 104;
const SYS_MKDIR: u64 = 105;
const SYS_UNLINK: u64 = 106;
const SYS_READDIR: u64 = 107;
const SYS_CHDIR: u64 = 108;
const SYS_GETCWD: u64 = 109;

// Process operations (110-119)
const SYS_FORK: u64 = 110;
const SYS_EXEC: u64 = 111;
const SYS_WAITPID: u64 = 112;
const SYS_BRK: u64 = 113;
const SYS_GETPPID: u64 = 114;
const SYS_KILL: u64 = 115;

// Memory operations (120-124)
const SYS_MMAP: u64 = 120;
const SYS_MUNMAP: u64 = 121;
const SYS_MPROTECT: u64 = 122;

// Network operations (130-139)
const SYS_SOCKET: u64 = 130;
const SYS_CONNECT: u64 = 131;
const SYS_SENDTO: u64 = 132;
const SYS_RECVFROM: u64 = 133;
const SYS_BIND: u64 = 134;
const SYS_LISTEN: u64 = 135;
const SYS_ACCEPT: u64 = 136;

// Time operations (140-144)
const SYS_TIME: u64 = 140;
const SYS_NANOSLEEP: u64 = 141;
const SYS_CLOCK_GETTIME: u64 = 142;

// Misc operations (150-159)
const SYS_IOCTL: u64 = 150;
const SYS_PIPE: u64 = 151;
const SYS_DUP2: u64 = 152;

/// Safely read a string from user memory address.
fn read_user_str(ptr: u64, len: u64) -> Option<String> {
    if ptr == 0 || len == 0 || len > 4096 {
        return None;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(slice).ok().map(|s| String::from(s))
}

/// Write bytes to user memory. Returns number of bytes written.
unsafe fn write_user_buf(ptr: u64, data: &[u8], max_len: u64) -> usize {
    let len = data.len().min(max_len as usize);
    let dst = core::slice::from_raw_parts_mut(ptr as *mut u8, len);
    dst.copy_from_slice(&data[..len]);
    len
}

pub fn dispatch(syscall_num: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    // Seccomp filter check
    let pid = task::current_pid();
    match crate::capability::seccomp_check(pid, syscall_num) {
        crate::capability::FilterAction::Kill => {
            serial_println!("[seccomp] pid {} killed: blocked syscall {}", pid, syscall_num);
            klog_println!("[seccomp] pid {} killed: blocked syscall {}", pid, syscall_num);
            crate::capability::audit_log(crate::capability::AuditEvent::SeccompViolation {
                pid,
                syscall: syscall_num,
            });
            task::exit();
        }
        crate::capability::FilterAction::Log => {
            serial_println!("[seccomp] pid {} logged syscall {}", pid, syscall_num);
        }
        crate::capability::FilterAction::Allow => {}
    }

    // Reset return value
    set_retval(0);

    // Syscall latency tracking
    let stats_start = crate::syscall_stats::begin();

    match syscall_num {
        SYS_WRITE => {
            let buf = arg1 as *const u8;
            let len = arg2 as usize;
            if buf.is_null() || len > 4096 {
                serial_println!("[syscall] write: invalid args");
                return 0;
            }
            let slice = unsafe { core::slice::from_raw_parts(buf, len) };
            if let Ok(s) = core::str::from_utf8(slice) {
                serial_println!("[user] {}", s);
                println!("[user] {}", s);
            }
        }
        SYS_EXIT => {
            let code = arg1 as i32;
            serial_println!("[syscall] exit({})", code);
            let user_pid = crate::userspace::current_process();
            if let Some(pid) = user_pid {
                crate::userspace::exit_process(pid, code);
                serial_println!("[userspace] process pid={} exited with code {}", pid, code);
                crate::userspace::return_to_kernel();
                // return_to_kernel sets flag — just return from syscall handler
                // The iret will go back to user code's jmp$ loop
                // Keyboard interrupts will still reach the shell
                return 0;
            }
            klog_println!("[syscall] process exited with code {}", code);
            task::exit();
        }
        SYS_YIELD => {
            task::yield_now();
        }
        SYS_GETPID => {
            let pid = task::current_pid();
            set_retval(pid as i64);
            serial_println!("[syscall] getpid() = {}", pid);
        }
        SYS_SLEEP => {
            let duration = arg1;
            let target = timer::ticks() + duration;
            while timer::ticks() < target {
                task::yield_now();
            }
        }
        SYS_SEND => {
            let channel_id = arg1 as usize;
            let byte = arg2 as u8;
            ipc::send(channel_id, byte);
        }
        SYS_RECV => {
            let channel_id = arg1 as usize;
            // Spin-yield until data is available
            loop {
                if let Some(_byte) = ipc::recv(channel_id) {
                    break;
                }
                task::yield_now();
            }
        }
        SYS_GETUID => {
            let uid = crate::security::current_uid();
            serial_println!("[syscall] getuid() = {}", uid);
        }
        SYS_SETUID => {
            let target_uid = arg1 as u32;
            let pid = task::current_pid();
            if crate::capability::has_cap(pid, crate::capability::CAP_SETUID) {
                let _ = crate::security::set_current_uid(target_uid);
                serial_println!("[syscall] setuid({}) ok", target_uid);
            } else {
                serial_println!("[syscall] setuid({}) denied — no CAP_SETUID", target_uid);
                crate::capability::audit_log(crate::capability::AuditEvent::CapabilityDenied {
                    pid,
                    cap: "CAP_SETUID",
                });
            }
        }
        SYS_GETGID => {
            let gid = crate::security::current_gid();
            serial_println!("[syscall] getgid() = {}", gid);
        }
        SYS_SETGID => {
            let _target_gid = arg1 as u32;
            serial_println!("[syscall] setgid() — not fully implemented");
        }
        SYS_GETGROUPS => {
            serial_println!("[syscall] getgroups() — not fully implemented");
        }
        SYS_CHMOD => {
            serial_println!("[syscall] chmod() — use shell command instead");
        }
        SYS_CHOWN => {
            serial_println!("[syscall] chown() — use shell command instead");
        }
        SYS_ACCESS => {
            serial_println!("[syscall] access() — use shell command instead");
        }

        // ── File operations ──────────────────────────────────────────

        SYS_OPEN => {
            // open(path_ptr, path_len, flags) → fd
            if let Some(path) = read_user_str(arg1, arg2) {
                match crate::fd::open(&path) {
                    Ok(fd) => {
                        serial_println!("[syscall] open({}) = fd {}", path, fd);
                    }
                    Err(e) => {
                        serial_println!("[syscall] open({}) failed: {}", path, e);
                    }
                }
            } else {
                serial_println!("[syscall] open: invalid path pointer");
            }
        }

        SYS_READ => {
            // read(fd, buf_ptr, len) → bytes_read
            let fd = arg1 as usize;
            let buf_ptr = arg2;
            let len = arg3 as usize;
            if buf_ptr == 0 || len == 0 || len > 4096 {
                serial_println!("[syscall] read: invalid args");
                return 0;
            }
            let mut tmp = alloc::vec![0u8; len];
            match crate::fd::read(fd, &mut tmp) {
                Ok(n) => {
                    unsafe { write_user_buf(buf_ptr, &tmp[..n], len as u64) };
                    serial_println!("[syscall] read(fd {}) = {} bytes", fd, n);
                }
                Err(e) => {
                    serial_println!("[syscall] read(fd {}) failed: {}", fd, e);
                }
            }
        }

        SYS_CLOSE => {
            // close(fd) → 0
            let fd = arg1 as usize;
            match crate::fd::close(fd) {
                Ok(()) => {
                    serial_println!("[syscall] close(fd {}) ok", fd);
                }
                Err(e) => {
                    serial_println!("[syscall] close(fd {}) failed: {}", fd, e);
                }
            }
        }

        SYS_STAT => {
            // stat(path_ptr, path_len, buf_ptr) → 0
            if let Some(path) = read_user_str(arg1, arg2) {
                // Verify the file exists via cat
                match crate::vfs::cat(&path) {
                    Ok(_) => {
                        serial_println!("[syscall] stat({}) ok", path);
                    }
                    Err(e) => {
                        serial_println!("[syscall] stat({}) failed: {}", path, e);
                    }
                }
            } else {
                serial_println!("[syscall] stat: invalid path pointer");
            }
        }

        SYS_LSEEK => {
            // lseek(fd, offset, whence) → new_offset
            let fd = arg1;
            let offset = arg2;
            let whence = arg3;
            serial_println!("[syscall] lseek(fd {}, offset {}, whence {}) — stub, returning 0", fd, offset, whence);
        }

        SYS_MKDIR => {
            // mkdir(path_ptr, path_len) → 0
            if let Some(path) = read_user_str(arg1, arg2) {
                match crate::vfs::mkdir(&path) {
                    Ok(()) => {
                        serial_println!("[syscall] mkdir({}) ok", path);
                    }
                    Err(e) => {
                        serial_println!("[syscall] mkdir({}) failed: {}", path, e);
                    }
                }
            } else {
                serial_println!("[syscall] mkdir: invalid path pointer");
            }
        }

        SYS_UNLINK => {
            // unlink(path_ptr, path_len) → 0
            if let Some(path) = read_user_str(arg1, arg2) {
                match crate::vfs::rm(&path) {
                    Ok(()) => {
                        serial_println!("[syscall] unlink({}) ok", path);
                    }
                    Err(e) => {
                        serial_println!("[syscall] unlink({}) failed: {}", path, e);
                    }
                }
            } else {
                serial_println!("[syscall] unlink: invalid path pointer");
            }
        }

        SYS_READDIR => {
            // readdir(path_ptr, path_len, buf_ptr, buf_len) → bytes
            if let Some(path) = read_user_str(arg1, arg2) {
                match crate::vfs::ls(&path) {
                    Ok(entries) => {
                        // Format entries as newline-separated names
                        let mut output = String::new();
                        for (name, kind) in &entries {
                            output.push_str(name);
                            output.push(if *kind == 'd' { '/' } else { ' ' });
                            output.push('\n');
                        }
                        if arg3 != 0 && entries.len() > 0 {
                            unsafe { write_user_buf(arg3, output.as_bytes(), 4096) };
                        }
                        serial_println!("[syscall] readdir({}) = {} entries", path, entries.len());
                    }
                    Err(e) => {
                        serial_println!("[syscall] readdir({}) failed: {}", path, e);
                    }
                }
            } else {
                serial_println!("[syscall] readdir: invalid path pointer");
            }
        }

        SYS_CHDIR => {
            // chdir(path_ptr, path_len) → 0
            if let Some(path) = read_user_str(arg1, arg2) {
                crate::env::set("PWD", &path);
                serial_println!("[syscall] chdir({}) ok", path);
            } else {
                serial_println!("[syscall] chdir: invalid path pointer");
            }
        }

        SYS_GETCWD => {
            // getcwd(buf_ptr, buf_len) → len
            let buf_ptr = arg1;
            let buf_len = arg2;
            let cwd = crate::env::get("PWD").unwrap_or_else(|| String::from("/"));
            if buf_ptr != 0 && buf_len > 0 {
                let n = unsafe { write_user_buf(buf_ptr, cwd.as_bytes(), buf_len) };
                serial_println!("[syscall] getcwd() = {} ({} bytes)", cwd, n);
            } else {
                serial_println!("[syscall] getcwd: invalid buffer");
            }
        }

        // ── Process operations ───────────────────────────────────────

        SYS_FORK => {
            serial_println!("[syscall] fork() — not implemented, returning -1");
        }

        SYS_EXEC => {
            if let Some(path) = read_user_str(arg1, arg2) {
                serial_println!("[syscall] exec({}) — not implemented, returning -1", path);
            } else {
                serial_println!("[syscall] exec: invalid path, returning -1");
            }
        }

        SYS_WAITPID => {
            let wait_pid = arg1;
            serial_println!("[syscall] waitpid({}) — not implemented, returning -1", wait_pid);
        }

        SYS_BRK => {
            let addr = arg1;
            serial_println!("[syscall] brk(0x{:x}) — stub, returning current brk", addr);
        }

        SYS_GETPPID => {
            // Return 0 (kernel) as parent for now
            serial_println!("[syscall] getppid() = 0 (stub)");
        }

        SYS_KILL => {
            let target_pid = arg1;
            let signal = arg2;
            serial_println!("[syscall] kill(pid {}, sig {}) — stub", target_pid, signal);
        }

        // ── Memory operations ────────────────────────────────────────

        SYS_MMAP => {
            let addr = arg1;
            let len = arg2;
            serial_println!("[syscall] mmap(0x{:x}, {}) — not implemented, returning -1", addr, len);
        }

        SYS_MUNMAP => {
            let addr = arg1;
            let len = arg2;
            serial_println!("[syscall] munmap(0x{:x}, {}) — not implemented, returning -1", addr, len);
        }

        SYS_MPROTECT => {
            let addr = arg1;
            let len = arg2;
            let prot = arg3;
            serial_println!("[syscall] mprotect(0x{:x}, {}, 0x{:x}) — not implemented, returning -1", addr, len, prot);
        }

        // ── Network operations ───────────────────────────────────────

        SYS_SOCKET => {
            let domain = arg1;
            let sock_type = arg2;
            let protocol = arg3;
            serial_println!("[syscall] socket({}, {}, {}) — not implemented, returning -1", domain, sock_type, protocol);
        }

        SYS_CONNECT => {
            let fd = arg1;
            serial_println!("[syscall] connect(fd {}) — not implemented, returning -1", fd);
        }

        SYS_SENDTO => {
            let fd = arg1;
            let len = arg3;
            serial_println!("[syscall] sendto(fd {}, {} bytes) — not implemented, returning -1", fd, len);
        }

        SYS_RECVFROM => {
            let fd = arg1;
            let len = arg3;
            serial_println!("[syscall] recvfrom(fd {}, {} bytes) — not implemented, returning -1", fd, len);
        }

        SYS_BIND => {
            let fd = arg1;
            serial_println!("[syscall] bind(fd {}) — not implemented, returning -1", fd);
        }

        SYS_LISTEN => {
            let fd = arg1;
            let backlog = arg2;
            serial_println!("[syscall] listen(fd {}, backlog {}) — not implemented, returning -1", fd, backlog);
        }

        SYS_ACCEPT => {
            let fd = arg1;
            serial_println!("[syscall] accept(fd {}) — not implemented, returning -1", fd);
        }

        // ── Time operations ──────────────────────────────────────────

        SYS_TIME => {
            let secs = timer::uptime_secs();
            set_retval(secs as i64);
            serial_println!("[syscall] time() = {} seconds since boot", secs);
        }

        SYS_NANOSLEEP => {
            // nanosleep(ms) — sleep for N milliseconds
            let ms = arg1;
            let ticks_to_wait = (ms * timer::PIT_FREQUENCY_HZ) / 1000;
            let target = timer::ticks() + ticks_to_wait.max(1);
            while timer::ticks() < target {
                task::yield_now();
            }
            serial_println!("[syscall] nanosleep({} ms) done", ms);
        }

        SYS_CLOCK_GETTIME => {
            // clock_gettime(buf_ptr) — write seconds+ticks to buffer
            let buf_ptr = arg1;
            if buf_ptr != 0 {
                let secs = timer::uptime_secs();
                let ticks = timer::ticks();
                // Write as two u64 values: [seconds, ticks]
                let data: [u64; 2] = [secs, ticks];
                let bytes = unsafe {
                    core::slice::from_raw_parts(data.as_ptr() as *const u8, 16)
                };
                unsafe { write_user_buf(buf_ptr, bytes, 16) };
                serial_println!("[syscall] clock_gettime() = {}s, {} ticks", secs, ticks);
            } else {
                serial_println!("[syscall] clock_gettime: null buffer");
            }
        }

        // ── Misc operations ─────────────────────────────────────────

        SYS_IOCTL => {
            let fd = arg1;
            let request = arg2;
            serial_println!("[syscall] ioctl(fd {}, req 0x{:x}) — not implemented, returning -1", fd, request);
        }

        SYS_PIPE => {
            serial_println!("[syscall] pipe() — not implemented, returning -1");
        }

        SYS_DUP2 => {
            let oldfd = arg1;
            let newfd = arg2;
            serial_println!("[syscall] dup2({}, {}) — not implemented, returning -1", oldfd, newfd);
        }

        _ => {
            serial_println!("[syscall] unknown syscall {}", syscall_num);
        }
    }

    // Record syscall latency
    crate::syscall_stats::end(syscall_num, stats_start);
    SYSCALL_RETVAL.load(core::sync::atomic::Ordering::SeqCst)
}

/// Syscall return value — set by handlers, read by trampoline.
static SYSCALL_RETVAL: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

/// Set syscall return value (called from within dispatch match arms).
fn set_retval(val: i64) {
    SYSCALL_RETVAL.store(val, core::sync::atomic::Ordering::SeqCst);
}
