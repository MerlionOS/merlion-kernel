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

use crate::{serial_println, klog_println, println, task, timer, ipc};

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 1;
const SYS_YIELD: u64 = 2;
const SYS_GETPID: u64 = 3;
const SYS_SLEEP: u64 = 4;
const SYS_SEND: u64 = 5;
const SYS_RECV: u64 = 6;

pub fn dispatch(syscall_num: u64, arg1: u64, arg2: u64, _arg3: u64) {
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
        _ => {
            serial_println!("[syscall] unknown syscall {}", syscall_num);
        }
    }
}
