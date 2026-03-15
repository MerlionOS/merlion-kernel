/// Syscall dispatch (int 0x80).
/// ABI: rax = syscall number, rdi = arg1, rsi = arg2, rdx = arg3.
///
/// Syscalls:
///   0 (SYS_WRITE): write(buf: *const u8, len: usize) — print to serial+VGA
///   1 (SYS_EXIT):  exit(code: usize) — terminate current task
///   2 (SYS_YIELD): yield() — yield to scheduler

use crate::{serial_println, klog_println, println, task};

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 1;
const SYS_YIELD: u64 = 2;

/// Called from the interrupt handler with the user's register values.
pub fn dispatch(syscall_num: u64, arg1: u64, arg2: u64, _arg3: u64) {
    match syscall_num {
        SYS_WRITE => {
            let buf = arg1 as *const u8;
            let len = arg2 as usize;
            // Validate the pointer is not null and length is reasonable
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
        _ => {
            serial_println!("[syscall] unknown syscall {}", syscall_num);
        }
    }
}
