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

// Libc support (160-169) — U5
const SYS_PRINTF: u64 = 160;

// Dynamic linking (170-179) — U6
const SYS_DLOPEN: u64 = 170;
const SYS_DLSYM: u64 = 171;
const SYS_DLCLOSE: u64 = 172;

// Signal handling (180-184)
const SYS_SIGACTION: u64 = 180;
const SYS_SIGRETURN: u64 = 181;

// Threads & IPC (190-199)
const SYS_CLONE: u64 = 190;
const SYS_SHMGET: u64 = 191;
const SYS_SHMAT: u64 = 192;
const SYS_SHMDT: u64 = 193;
const SYS_TTY_READ: u64 = 194;
const SYS_FWRITE: u64 = 195;
const SYS_FBWRITE: u64 = 196;
const SYS_WGET: u64 = 197;

// Audio & Hardware (200-209)
const SYS_BEEP: u64 = 200;
const SYS_PLAY_TONE: u64 = 201;
const SYS_DISK_READ: u64 = 202;
const SYS_DISK_WRITE: u64 = 203;
const SYS_CPUINFO: u64 = 204;
const SYS_USB_LIST: u64 = 205;

// GUI (210-219)
const SYS_WIN_CREATE: u64 = 210;
const SYS_WIN_PIXEL: u64 = 211;
const SYS_WIN_TEXT: u64 = 212;
const SYS_WIN_CLOSE: u64 = 213;

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
            set_retval(len as i64);
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
            set_retval(uid as i64);
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
            set_retval(gid as i64);
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
                let flags = arg3 as u32;
                // Use per-process fd table if user process is active
                if let Some(pid) = crate::userspace::current_process() {
                    match crate::userspace::proc_open(pid, &path, flags) {
                        Ok(fd) => {
                            serial_println!("[syscall] open({}) = fd {} (pid {})", path, fd, pid);
                            set_retval(fd as i64);
                        }
                        Err(e) => {
                            serial_println!("[syscall] open({}) failed: {}", path, e);
                            set_retval(-1);
                        }
                    }
                } else {
                    match crate::fd::open(&path) {
                        Ok(fd) => {
                            serial_println!("[syscall] open({}) = fd {}", path, fd);
                            set_retval(fd as i64);
                        }
                        Err(e) => {
                            serial_println!("[syscall] open({}) failed: {}", path, e);
                            set_retval(-1);
                        }
                    }
                }
            } else {
                serial_println!("[syscall] open: invalid path pointer");
                set_retval(-1);
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
            let result = if let Some(pid) = crate::userspace::current_process() {
                crate::userspace::proc_read(pid, fd, &mut tmp)
            } else {
                crate::fd::read(fd, &mut tmp)
            };
            match result {
                Ok(n) => {
                    unsafe { write_user_buf(buf_ptr, &tmp[..n], len as u64) };
                    serial_println!("[syscall] read(fd {}) = {} bytes", fd, n);
                    set_retval(n as i64);
                }
                Err(e) => {
                    serial_println!("[syscall] read(fd {}) failed: {}", fd, e);
                    set_retval(-1);
                }
            }
        }

        SYS_CLOSE => {
            // close(fd) → 0
            let fd = arg1 as usize;
            let result = if let Some(pid) = crate::userspace::current_process() {
                crate::userspace::proc_close(pid, fd).map(|_| ())
            } else {
                crate::fd::close(fd)
            };
            match result {
                Ok(()) => {
                    serial_println!("[syscall] close(fd {}) ok", fd);
                    set_retval(0);
                }
                Err(e) => {
                    serial_println!("[syscall] close(fd {}) failed: {}", fd, e);
                    set_retval(-1);
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
                        set_retval(0);
                    }
                    Err(e) => {
                        serial_println!("[syscall] stat({}) failed: {}", path, e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] stat: invalid path pointer");
                set_retval(-1);
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
                        set_retval(0);
                    }
                    Err(e) => {
                        serial_println!("[syscall] mkdir({}) failed: {}", path, e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] mkdir: invalid path pointer");
                set_retval(-1);
            }
        }

        SYS_UNLINK => {
            // unlink(path_ptr, path_len) → 0
            if let Some(path) = read_user_str(arg1, arg2) {
                match crate::vfs::rm(&path) {
                    Ok(()) => {
                        serial_println!("[syscall] unlink({}) ok", path);
                        set_retval(0);
                    }
                    Err(e) => {
                        serial_println!("[syscall] unlink({}) failed: {}", path, e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] unlink: invalid path pointer");
                set_retval(-1);
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
                        set_retval(entries.len() as i64);
                    }
                    Err(e) => {
                        serial_println!("[syscall] readdir({}) failed: {}", path, e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] readdir: invalid path pointer");
                set_retval(-1);
            }
        }

        SYS_CHDIR => {
            // chdir(path_ptr, path_len) → 0
            if let Some(path) = read_user_str(arg1, arg2) {
                crate::env::set("PWD", &path);
                serial_println!("[syscall] chdir({}) ok", path);
                set_retval(0);
            } else {
                serial_println!("[syscall] chdir: invalid path pointer");
                set_retval(-1);
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
                set_retval(n as i64);
            } else {
                serial_println!("[syscall] getcwd: invalid buffer");
                set_retval(-1);
            }
        }

        // ── Process operations ───────────────────────────────────────

        SYS_FORK => {
            if let Some(parent_pid) = crate::userspace::current_process() {
                match crate::userspace::fork_process(parent_pid) {
                    Ok(child_pid) => {
                        serial_println!("[syscall] fork() = {} (parent {})", child_pid, parent_pid);
                        set_retval(child_pid as i64);
                    }
                    Err(e) => {
                        serial_println!("[syscall] fork() failed: {}", e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] fork() — no user process context");
                set_retval(-1);
            }
        }

        SYS_EXEC => {
            if let Some(path) = read_user_str(arg1, arg2) {
                serial_println!("[syscall] exec({})", path);
                // Check if it's a built-in program
                if let Some(elf_data) = crate::userspace::get_builtin_program(&path) {
                    // Create new process and enter it
                    match crate::userspace::create_process(&path, &elf_data) {
                        Ok(pid) => {
                            // Mark old process as exited
                            if let Some(old_pid) = crate::userspace::current_process() {
                                crate::userspace::exit_process(old_pid, 0);
                            }
                            serial_println!("[syscall] exec: switching to pid {}", pid);
                            crate::userspace::enter_userspace(pid);
                            // never returns
                        }
                        Err(e) => {
                            serial_println!("[syscall] exec failed: {}", e);
                            set_retval(-1);
                        }
                    }
                } else {
                    serial_println!("[syscall] exec: program '{}' not found", path);
                    set_retval(-1);
                }
            } else {
                serial_println!("[syscall] exec: invalid path");
                set_retval(-1);
            }
        }

        SYS_WAITPID => {
            let wait_pid = arg1 as u32;
            match crate::userspace::waitpid_blocking(wait_pid) {
                Ok(code) => {
                    serial_println!("[syscall] waitpid({}) = {} (exited)", wait_pid, code);
                    set_retval(code as i64);
                }
                Err(e) => {
                    serial_println!("[syscall] waitpid({}) — {}", wait_pid, e);
                    set_retval(-1);
                }
            }
        }

        SYS_BRK => {
            let new_brk = arg1;
            if let Some(pid) = crate::userspace::current_process() {
                let result = crate::userspace::handle_brk(pid, new_brk);
                serial_println!("[syscall] brk(0x{:x}) = 0x{:x} (pid {})", new_brk, result, pid);
                set_retval(result as i64);
            } else {
                serial_println!("[syscall] brk(0x{:x}) — no user process", new_brk);
                set_retval(-1);
            }
        }

        SYS_GETPPID => {
            // Return 0 (kernel) as parent for now
            serial_println!("[syscall] getppid() = 0 (stub)");
            set_retval(0);
        }

        SYS_KILL => {
            let target_pid = arg1 as usize;
            let signal = arg2 as u8;
            let _ = crate::sighandler::deliver_signal(target_pid, signal);
            serial_println!("[syscall] kill(pid {}, sig {})", target_pid, signal);
            set_retval(0);
        }

        // ── Memory operations ────────────────────────────────────────

        SYS_MMAP => {
            // mmap(addr_hint, len, prot_flags) → mapped_addr
            // Anonymous mapping only (no file-backed)
            let addr_hint = arg1;
            let len = arg2;
            if len == 0 || len > 0x1000_0000 {
                serial_println!("[syscall] mmap: invalid length {}", len);
                set_retval(-1);
            } else {
                let num_pages = (len + 0xFFF) / 4096;
                // Use a region above the heap
                let base = if addr_hint != 0 && (addr_hint & 0xFFF) == 0 {
                    addr_hint
                } else {
                    // Auto-allocate from mmap region (0x1000_0000 upward)
                    static MMAP_NEXT: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0x0000_0100_0000);
                    MMAP_NEXT.fetch_add(num_pages * 4096, core::sync::atomic::Ordering::SeqCst)
                };
                #[cfg(target_arch = "x86_64")]
                {
                    use x86_64::structures::paging::{Page, PageTableFlags};
                    use x86_64::VirtAddr;
                    let user_rw = PageTableFlags::PRESENT
                        | PageTableFlags::WRITABLE
                        | PageTableFlags::USER_ACCESSIBLE;
                    let mut ok = true;
                    for i in 0..num_pages {
                        let page = Page::containing_address(VirtAddr::new(base + i * 4096));
                        if crate::memory::map_page_global(page, user_rw).is_none() {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        serial_println!("[syscall] mmap({:#x}, {}) = {:#x} ({} pages)",
                            addr_hint, len, base, num_pages);
                        set_retval(base as i64);
                    } else {
                        serial_println!("[syscall] mmap: allocation failed");
                        set_retval(-1);
                    }
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    serial_println!("[syscall] mmap: not supported on this arch");
                    set_retval(-1);
                }
            }
        }

        SYS_MUNMAP => {
            let addr = arg1;
            let len = arg2;
            let num_pages = (len + 0xFFF) / 4096;
            // For now, just acknowledge — actual unmapping requires page table walk
            serial_println!("[syscall] munmap({:#x}, {}) — {} pages freed (logical)", addr, len, num_pages);
            set_retval(0);
        }

        SYS_MPROTECT => {
            let addr = arg1;
            let len = arg2;
            let prot = arg3;
            // Acknowledge the request — actual protection change requires PTE modification
            serial_println!("[syscall] mprotect({:#x}, {}, {:#x}) — acknowledged", addr, len, prot);
            set_retval(0);
        }

        // ── Network operations ───────────────────────────────────────

        SYS_SOCKET => {
            // socket(domain, type, protocol) -> fd
            // For now, always create a TCP socket
            let domain = arg1; // AF_INET = 2
            let sock_type = arg2; // SOCK_STREAM = 1
            serial_println!("[syscall] socket({}, {}, {}) = fd 10", domain, sock_type, arg3);
            // Allocate a fake fd for the socket
            set_retval(10); // fixed fd for now
        }

        SYS_CONNECT => {
            // connect(fd, addr_ptr, addr_len)
            // Read IP:port from user memory
            let fd = arg1;
            if arg2 != 0 && arg3 >= 8 {
                let addr_slice = unsafe { core::slice::from_raw_parts(arg2 as *const u8, arg3 as usize) };
                // Parse sockaddr_in: family(2) + port(2) + ip(4)
                let port = u16::from_be_bytes([addr_slice[2], addr_slice[3]]);
                let ip = [addr_slice[4], addr_slice[5], addr_slice[6], addr_slice[7]];
                serial_println!("[syscall] connect(fd {}, {}.{}.{}.{}:{})", fd, ip[0], ip[1], ip[2], ip[3], port);
                // Try to connect using our TCP stack
                match crate::tcp_real::connect(crate::net::Ipv4Addr(ip), port) {
                    Ok(conn_id) => {
                        serial_println!("[syscall] connect: TCP connection {} established", conn_id);
                        set_retval(0);
                    }
                    Err(e) => {
                        serial_println!("[syscall] connect failed: {}", e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] connect: invalid addr");
                set_retval(-1);
            }
        }

        SYS_SENDTO => {
            // sendto(fd, buf_ptr, len) -> bytes_sent
            let fd = arg1;
            let buf_ptr = arg2;
            let len = arg3 as usize;
            if buf_ptr != 0 && len > 0 && len <= 4096 {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
                // For now, just log the data
                if let Ok(s) = core::str::from_utf8(data) {
                    serial_println!("[syscall] sendto(fd {}, {} bytes): {}", fd, len, s.trim());
                } else {
                    serial_println!("[syscall] sendto(fd {}, {} bytes binary)", fd, len);
                }
                set_retval(len as i64);
            } else {
                set_retval(-1);
            }
        }

        SYS_RECVFROM => {
            // recvfrom(fd, buf_ptr, len) -> bytes_received
            let fd = arg1;
            let buf_ptr = arg2;
            let len = arg3 as usize;
            serial_println!("[syscall] recvfrom(fd {}, buf={:#x}, len={}) — returning 0 (no data)", fd, buf_ptr, len);
            set_retval(0); // no data available
        }

        SYS_BIND => {
            let fd = arg1;
            serial_println!("[syscall] bind(fd {}) — not implemented, returning -1", fd);
            set_retval(-1);
        }

        SYS_LISTEN => {
            let fd = arg1;
            let backlog = arg2;
            serial_println!("[syscall] listen(fd {}, backlog {}) — not implemented, returning -1", fd, backlog);
            set_retval(-1);
        }

        SYS_ACCEPT => {
            let fd = arg1;
            serial_println!("[syscall] accept(fd {}) — not implemented, returning -1", fd);
            set_retval(-1);
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
            set_retval(0);
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
                set_retval(0);
            } else {
                serial_println!("[syscall] clock_gettime: null buffer");
                set_retval(-1);
            }
        }

        // ── Misc operations ─────────────────────────────────────────

        SYS_IOCTL => {
            let fd = arg1;
            let request = arg2;
            serial_println!("[syscall] ioctl(fd {}, req 0x{:x}) — not implemented, returning -1", fd, request);
            set_retval(-1);
        }

        SYS_PIPE => {
            // pipe(fds_ptr) — creates pipe, writes [read_fd, write_fd] to user memory
            let fds_ptr = arg1;
            if fds_ptr == 0 {
                serial_println!("[syscall] pipe: null pointer");
                set_retval(-1);
            } else {
                match crate::pipefs::create_pipe() {
                    Ok((read_fd, write_fd)) => {
                        // Write [read_fd, write_fd] as two u64 values to user memory
                        let data: [u64; 2] = [read_fd as u64, write_fd as u64];
                        let bytes = unsafe {
                            core::slice::from_raw_parts(data.as_ptr() as *const u8, 16)
                        };
                        unsafe { write_user_buf(fds_ptr, bytes, 16) };
                        serial_println!("[syscall] pipe() = [{}, {}]", read_fd, write_fd);
                        set_retval(0);
                    }
                    Err(e) => {
                        serial_println!("[syscall] pipe() failed: {}", e);
                        set_retval(-1);
                    }
                }
            }
        }

        SYS_DUP2 => {
            // dup2(oldfd, newfd) → newfd
            let oldfd = arg1 as usize;
            let newfd = arg2 as usize;
            if let Some(pid) = crate::userspace::current_process() {
                match crate::userspace::proc_dup2(pid, oldfd, newfd) {
                    Ok(fd) => {
                        serial_println!("[syscall] dup2({}, {}) = {}", oldfd, newfd, fd);
                        set_retval(fd as i64);
                    }
                    Err(e) => {
                        serial_println!("[syscall] dup2({}, {}) failed: {}", oldfd, newfd, e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] dup2: no user process");
                set_retval(-1);
            }
        }

        // ── Libc support (U5) ────────────────────────────────────────

        SYS_PRINTF => {
            // printf(fmt_ptr, fmt_len, int_arg)
            // Supports: %d (decimal), %x (hex), %s (string at int_arg), %%
            if let Some(fmt) = read_user_str(arg1, arg2) {
                let int_arg = arg3;
                let mut output = String::new();
                let bytes = fmt.as_bytes();
                let mut i = 0;
                let mut arg_used = false;
                while i < bytes.len() {
                    if bytes[i] == b'%' && i + 1 < bytes.len() {
                        i += 1;
                        match bytes[i] {
                            b'd' => {
                                use core::fmt::Write;
                                let val = if arg_used { 0 } else { int_arg };
                                arg_used = true;
                                let _ = write!(output, "{}", val as i64);
                            }
                            b'u' => {
                                use core::fmt::Write;
                                let val = if arg_used { 0 } else { int_arg };
                                arg_used = true;
                                let _ = write!(output, "{}", val);
                            }
                            b'x' => {
                                use core::fmt::Write;
                                let val = if arg_used { 0 } else { int_arg };
                                arg_used = true;
                                let _ = write!(output, "{:x}", val);
                            }
                            b's' => {
                                // int_arg is a pointer to null-terminated string
                                if !arg_used && int_arg != 0 {
                                    // Read up to 256 bytes from user memory
                                    let s = unsafe {
                                        let ptr = int_arg as *const u8;
                                        let mut len = 0;
                                        while len < 256 {
                                            if *ptr.add(len) == 0 { break; }
                                            len += 1;
                                        }
                                        core::str::from_utf8(
                                            core::slice::from_raw_parts(ptr, len)
                                        ).unwrap_or("(invalid)")
                                    };
                                    output.push_str(s);
                                    arg_used = true;
                                } else {
                                    output.push_str("(null)");
                                }
                            }
                            b'%' => output.push('%'),
                            other => {
                                output.push('%');
                                output.push(other as char);
                            }
                        }
                    } else {
                        output.push(bytes[i] as char);
                    }
                    i += 1;
                }
                let len = output.len();
                serial_println!("[user] {}", output);
                println!("[user] {}", output);
                set_retval(len as i64);
            } else {
                serial_println!("[syscall] printf: invalid format string");
                set_retval(-1);
            }
        }

        // ── Signal handling ────────────────────────────────────────

        SYS_SIGACTION => {
            // sigaction(signal, handler_type) → 0
            // handler_type: 0=default, 1=ignore, 2+=custom handler addr (future)
            let sig = arg1 as u8;
            let handler_type = arg2;
            let handler = match handler_type {
                0 => crate::sighandler::HandlerType::Default,
                1 => crate::sighandler::HandlerType::Ignore,
                _ => {
                    serial_println!("[syscall] sigaction: custom handlers not yet supported from userspace");
                    crate::sighandler::HandlerType::Default
                }
            };
            match crate::sighandler::register_handler(pid as usize, sig, handler) {
                Ok(()) => {
                    serial_println!("[syscall] sigaction(sig {}, type {}) ok", sig, handler_type);
                    set_retval(0);
                }
                Err(e) => {
                    serial_println!("[syscall] sigaction failed: {}", e);
                    set_retval(-1);
                }
            }
        }

        SYS_SIGRETURN => {
            serial_println!("[syscall] sigreturn — returning from signal handler");
            set_retval(0);
        }

        // ── Dynamic linking (U6) ───────────────────────────────────

        SYS_DLOPEN => {
            // dlopen(name_ptr, name_len) → handle
            if let Some(name) = read_user_str(arg1, arg2) {
                let handle = crate::dynlink::dlopen(&name);
                serial_println!("[syscall] dlopen({}) = {}", name, handle);
                set_retval(handle as i64);
            } else {
                serial_println!("[syscall] dlopen: invalid name");
                set_retval(0);
            }
        }

        SYS_DLSYM => {
            // dlsym(handle, name_ptr, name_len) → func_addr
            let handle = arg1;
            if let Some(name) = read_user_str(arg2, arg3) {
                let addr = crate::dynlink::dlsym(handle, &name);
                serial_println!("[syscall] dlsym({}, {}) = {:#x}", handle, name, addr);
                set_retval(addr as i64);
            } else {
                serial_println!("[syscall] dlsym: invalid name");
                set_retval(0);
            }
        }

        SYS_DLCLOSE => {
            // dlclose(handle) → 0
            let handle = arg1;
            let result = crate::dynlink::dlclose(handle);
            serial_println!("[syscall] dlclose({}) = {}", handle, result);
            set_retval(result);
        }

        // ── Threads & IPC ─────────────────────────────────────────

        SYS_CLONE => {
            // clone(flags, stack_ptr) → child_tid
            // Creates a new thread sharing the parent's address space.
            let _flags = arg1;
            let stack_ptr = arg2;
            if let Some(parent_pid) = crate::userspace::current_process() {
                match crate::userspace::clone_thread(parent_pid, stack_ptr) {
                    Ok(child_tid) => {
                        serial_println!("[syscall] clone(stack={:#x}) = tid {}", stack_ptr, child_tid);
                        set_retval(child_tid as i64);
                    }
                    Err(e) => {
                        serial_println!("[syscall] clone failed: {}", e);
                        set_retval(-1);
                    }
                }
            } else {
                serial_println!("[syscall] clone: no user process");
                set_retval(-1);
            }
        }

        SYS_SHMGET => {
            // shmget(key, size) → shmid
            let _key = arg1 as u32;
            let size = arg2 as usize;
            let owner = if let Some(p) = crate::userspace::current_process() { p as usize } else { 0 };
            match crate::shmem::create_shmem("user_shm", size, owner) {
                Some(id) => {
                    serial_println!("[syscall] shmget(size={}) = {}", size, id);
                    set_retval(id as i64);
                }
                None => {
                    serial_println!("[syscall] shmget failed");
                    set_retval(-1);
                }
            }
        }

        SYS_SHMAT => {
            // shmat(shmid) → addr
            let shmid = arg1 as usize;
            let caller = if let Some(p) = crate::userspace::current_process() { p as usize } else { 0 };
            match crate::shmem::attach_shmem(shmid, caller) {
                Some(addr) => {
                    serial_println!("[syscall] shmat({}) = {:#x}", shmid, addr);
                    set_retval(addr as i64);
                }
                None => {
                    serial_println!("[syscall] shmat({}) failed", shmid);
                    set_retval(-1);
                }
            }
        }

        SYS_SHMDT => {
            // shmdt(shmid) → 0
            let shmid = arg1 as usize;
            let caller = if let Some(p) = crate::userspace::current_process() { p as usize } else { 0 };
            crate::shmem::detach_shmem(shmid, caller);
            serial_println!("[syscall] shmdt({}) — detached", shmid);
            set_retval(0);
        }

        SYS_TTY_READ => {
            // tty_read(buf_ptr, max_len) → bytes_read
            // Reads from TTY 0 input buffer
            let buf_ptr = arg1;
            let max_len = arg2 as usize;
            if buf_ptr != 0 && max_len > 0 {
                let mut tmp = alloc::vec![0u8; max_len.min(256)];
                match crate::tty::tty_read(0, &mut tmp) {
                    Ok(n) if n > 0 => {
                        unsafe { write_user_buf(buf_ptr, &tmp[..n], max_len as u64) };
                        serial_println!("[syscall] tty_read() = {} bytes", n);
                        set_retval(n as i64);
                    }
                    _ => {
                        set_retval(0); // no data available
                    }
                }
            } else {
                set_retval(-1);
            }
        }

        // ── File/Device I/O ────────────────────────────────────────

        SYS_FWRITE => {
            // fwrite(fd, buf_ptr, len) → bytes_written
            // Proper fd-based write (unlike SYS_WRITE which is stdout-only)
            let fd = arg1 as usize;
            let buf_ptr = arg2;
            let len = arg3 as usize;
            if buf_ptr == 0 || len == 0 || len > 4096 {
                set_retval(-1);
            } else {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };
                let result = if let Some(pid) = crate::userspace::current_process() {
                    crate::userspace::proc_write(pid, fd, data)
                } else {
                    crate::fd::write(fd, data)
                };
                match result {
                    Ok(n) => {
                        serial_println!("[syscall] fwrite(fd {}, {} bytes) = {}", fd, len, n);
                        set_retval(n as i64);
                    }
                    Err(e) => {
                        serial_println!("[syscall] fwrite failed: {}", e);
                        set_retval(-1);
                    }
                }
            }
        }

        SYS_FBWRITE => {
            // fbwrite(x, y, color) — set pixel in framebuffer
            // If x == 0xFFFF, y == 0xFFFF: render the framebuffer to screen
            let x = arg1 as usize;
            let y = arg2 as usize;
            let color = arg3 as u8;
            let mut fb = crate::framebuf::FRAMEBUF.lock();
            if x == 0xFFFF && y == 0xFFFF {
                fb.render();
                serial_println!("[syscall] fbwrite: render");
            } else {
                fb.set_pixel(x, y, color);
            }
            set_retval(0);
        }

        SYS_WGET => {
            // wget(url_ptr, url_len, buf_ptr) → bytes_received
            // Fetch URL and write response body to user buffer
            if let Some(url) = read_user_str(arg1, arg2) {
                match crate::wget_real::fetch(&url) {
                    Ok(body) => {
                        let buf_ptr = arg3;
                        if buf_ptr != 0 {
                            let n = unsafe {
                                write_user_buf(buf_ptr, body.as_bytes(), 4096)
                            };
                            serial_println!("[syscall] wget({}) = {} bytes", url, n);
                            set_retval(n as i64);
                        } else {
                            serial_println!("[user] {}", body);
                            println!("[user] {}", body);
                            set_retval(body.len() as i64);
                        }
                    }
                    Err(e) => {
                        serial_println!("[syscall] wget({}) failed: {}", url, e);
                        set_retval(-1);
                    }
                }
            } else {
                set_retval(-1);
            }
        }

        // ── Audio & Hardware ───────────────────────────────────────

        SYS_BEEP => {
            let freq = arg1 as u16;
            let duration = arg2 as u16;
            crate::audio::beep(freq, duration);
            serial_println!("[syscall] beep({} Hz, {} ms)", freq, duration);
            set_retval(0);
        }

        SYS_PLAY_TONE => {
            let freq = arg1 as u32;
            let duration = arg2 as u32;
            crate::audio_engine::play_tone(freq, duration);
            serial_println!("[syscall] play_tone({} Hz, {} ms)", freq, duration);
            set_retval(0);
        }

        SYS_DISK_READ => {
            // disk_read(sector, buf_ptr) → 0 or -1
            let sector = arg1;
            let buf_ptr = arg2;
            if buf_ptr != 0 {
                let mut tmp = [0u8; 512];
                match crate::virtio_blk::read_sector(sector, &mut tmp) {
                    Ok(()) => {
                        unsafe { write_user_buf(buf_ptr, &tmp, 512) };
                        serial_println!("[syscall] disk_read(sector {}) ok", sector);
                        set_retval(512);
                    }
                    Err(e) => {
                        serial_println!("[syscall] disk_read failed: {}", e);
                        set_retval(-1);
                    }
                }
            } else {
                set_retval(-1);
            }
        }

        SYS_DISK_WRITE => {
            // disk_write(sector, buf_ptr) → 0 or -1
            let sector = arg1;
            let buf_ptr = arg2;
            if buf_ptr != 0 {
                let data = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, 512) };
                let mut tmp = [0u8; 512];
                tmp.copy_from_slice(data);
                match crate::virtio_blk::write_sector(sector, &tmp) {
                    Ok(()) => {
                        serial_println!("[syscall] disk_write(sector {}) ok", sector);
                        set_retval(0);
                    }
                    Err(e) => {
                        serial_println!("[syscall] disk_write failed: {}", e);
                        set_retval(-1);
                    }
                }
            } else {
                set_retval(-1);
            }
        }

        SYS_CPUINFO => {
            // cpuinfo(buf_ptr, max_len) → bytes_written
            let buf_ptr = arg1;
            let max_len = arg2;
            let info = crate::smp::cpu_info_string();
            if buf_ptr != 0 {
                let n = unsafe { write_user_buf(buf_ptr, info.as_bytes(), max_len) };
                set_retval(n as i64);
            } else {
                serial_println!("[user] {}", info);
                println!("[user] {}", info);
                set_retval(info.len() as i64);
            }
        }

        SYS_USB_LIST => {
            // usb_list(buf_ptr, max_len) → bytes_written
            let buf_ptr = arg1;
            let max_len = arg2;
            let info = crate::xhci::info();
            if buf_ptr != 0 {
                let n = unsafe { write_user_buf(buf_ptr, info.as_bytes(), max_len) };
                set_retval(n as i64);
            } else {
                serial_println!("[user] {}", info);
                println!("[user] {}", info);
                set_retval(info.len() as i64);
            }
        }

        // ── GUI ────────────────────────────────────────────────────

        SYS_WIN_CREATE => {
            // win_create(width, height, title_ptr) → widget_id
            let _width = arg1;
            let _height = arg2;
            let id = crate::widget::create_widget(crate::widget::WidgetType::Panel {
                bg_color: crate::widget::Color::new(0, 0, 0),
                border: true,
            });
            serial_println!("[syscall] win_create() = {}", id);
            set_retval(id as i64);
        }

        SYS_WIN_PIXEL => {
            // win_pixel(widget_id_x_packed, y, color)
            // Draws on framebuffer — reuse FBWRITE logic
            let x = arg1 as usize;
            let y = arg2 as usize;
            let color = arg3 as u8;
            let mut fb = crate::framebuf::FRAMEBUF.lock();
            fb.set_pixel(x, y, color);
            set_retval(0);
        }

        SYS_WIN_TEXT => {
            // win_text(x, y, char) — draw character at position
            let x = arg1 as usize;
            let y = arg2 as usize;
            let ch = arg3 as u8;
            // Use VGA text buffer for simplicity
            if x < 80 && y < 25 {
                let idx = y * 80 + x;
                let vga = 0xb8000 as *mut u16;
                unsafe { *vga.add(idx) = (0x0F << 8) | ch as u16; }
            }
            set_retval(0);
        }

        SYS_WIN_CLOSE => {
            let id = arg1 as u32;
            crate::widget::destroy_widget(id);
            serial_println!("[syscall] win_close({})", id);
            set_retval(0);
        }

        _ => {
            serial_println!("[syscall] unknown syscall {}", syscall_num);
            set_retval(-1);
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
