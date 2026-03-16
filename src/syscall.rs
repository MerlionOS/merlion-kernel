/// Syscall dispatch (int 0x80).
/// ABI: rax = syscall number, rdi = arg1, rsi = arg2, rdx = arg3.
///
/// Syscalls:
///   0 (SYS_WRITE):  write(buf, len) — print to serial+VGA
///   1 (SYS_EXIT):   exit(code) — terminate current task
///   2 (SYS_YIELD):  yield() — yield to scheduler
///   3 (SYS_GETPID): getpid() — return current PID (in rax, future)
///   4 (SYS_SLEEP):  sleep(ticks) — busy-wait for N timer ticks
///   5 (SYS_SEND):   send(channel_id, byte) — send byte to IPC channel
///   6 (SYS_RECV):   recv(channel_id) — receive byte from IPC channel
///   7 (SYS_GETUID):    getuid() — return current UID
///   8 (SYS_SETUID):    setuid(uid) — set UID (requires CAP_SETUID)
///   9 (SYS_GETGID):    getgid() — return current GID
///  10 (SYS_SETGID):    setgid(gid) — set GID (requires CAP_SETGID)
///  11 (SYS_GETGROUPS): getgroups() — return group count
///  12 (SYS_CHMOD):     chmod(path_ptr, mode) — change file permissions
///  13 (SYS_CHOWN):     chown(path_ptr, uid_gid) — change ownership
///  14 (SYS_ACCESS):    access(path_ptr, mode) — check file access

use crate::{serial_println, klog_println, println, task, timer, ipc};

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

pub fn dispatch(syscall_num: u64, arg1: u64, arg2: u64, _arg3: u64) {
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

    // Syscall latency tracking
    let stats_start = crate::syscall_stats::begin();

    match syscall_num {
        SYS_WRITE => {
            let buf = arg1 as *const u8;
            let len = arg2 as usize;
            if buf.is_null() || len > 4096 {
                serial_println!("[syscall] write: invalid args");
                return;
            }
            let slice = unsafe { core::slice::from_raw_parts(buf, len) };
            if let Ok(s) = core::str::from_utf8(slice) {
                serial_println!("[user] {}", s);
                println!("[user] {}", s);
            }
        }
        SYS_EXIT => {
            let code = arg1 as usize;
            serial_println!("[syscall] exit({})", code);
            klog_println!("[syscall] process exited with code {}", code);
            task::exit();
        }
        SYS_YIELD => {
            task::yield_now();
        }
        SYS_GETPID => {
            let pid = task::current_pid();
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
        _ => {
            serial_println!("[syscall] unknown syscall {}", syscall_num);
        }
    }

    // Record syscall latency
    crate::syscall_stats::end(syscall_num, stats_start);
}
