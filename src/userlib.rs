/// User-space standard library.
/// Provides a mini libc-like interface for user programs:
/// memory operations, string formatting, and syscall wrappers.
/// This would be compiled into a separate merlion-user crate
/// in a future phase.

/// Simple memset.
pub fn memset(dest: *mut u8, val: u8, count: usize) {
    for i in 0..count {
        unsafe { dest.add(i).write_volatile(val) };
    }
}

/// Simple memcpy.
pub fn memcpy(dest: *mut u8, src: *const u8, count: usize) {
    for i in 0..count {
        unsafe { dest.add(i).write_volatile(src.add(i).read_volatile()) };
    }
}

/// Simple memcmp. Returns 0 if equal.
pub fn memcmp(a: *const u8, b: *const u8, count: usize) -> i32 {
    for i in 0..count {
        let (va, vb) = unsafe { (a.add(i).read(), b.add(i).read()) };
        if va != vb {
            return va as i32 - vb as i32;
        }
    }
    0
}

/// Simple strlen (null-terminated).
pub fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    while unsafe { s.add(len).read() } != 0 {
        len += 1;
    }
    len
}

/// Format a u64 into a decimal string buffer. Returns bytes written.
pub fn u64_to_str(mut val: u64, buf: &mut [u8]) -> usize {
    if val == 0 {
        if !buf.is_empty() { buf[0] = b'0'; }
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 20;
    while val > 0 {
        i -= 1;
        tmp[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    let len = 20 - i;
    let copy_len = len.min(buf.len());
    buf[..copy_len].copy_from_slice(&tmp[i..i + copy_len]);
    copy_len
}

/// Format a u64 as hex into a buffer. Returns bytes written.
pub fn u64_to_hex(mut val: u64, buf: &mut [u8]) -> usize {
    if val == 0 {
        if buf.len() >= 2 { buf[0] = b'0'; buf[1] = b'x'; buf[2] = b'0'; return 3; }
        return 0;
    }
    let hex = b"0123456789abcdef";
    let mut tmp = [0u8; 18]; // "0x" + 16 hex digits
    tmp[0] = b'0';
    tmp[1] = b'x';
    let mut i = 18;
    while val > 0 {
        i -= 1;
        tmp[i] = hex[(val & 0xF) as usize];
        val >>= 4;
    }
    // Shift to fill from position 2
    let hex_len = 18 - i;
    let total = 2 + hex_len;
    if total <= buf.len() {
        buf[0] = b'0';
        buf[1] = b'x';
        buf[2..2 + hex_len].copy_from_slice(&tmp[i..18]);
        total
    } else {
        0
    }
}

/// Mini printf-like: supports %d, %s, %x (very simplified).
/// Returns the formatted string length.
pub fn snprintf(buf: &mut [u8], fmt: &[u8], args: &[u64]) -> usize {
    let mut out_pos = 0;
    let mut fmt_pos = 0;
    let mut arg_idx = 0;

    while fmt_pos < fmt.len() && out_pos < buf.len() {
        if fmt[fmt_pos] == b'%' && fmt_pos + 1 < fmt.len() {
            fmt_pos += 1;
            match fmt[fmt_pos] {
                b'd' => {
                    if arg_idx < args.len() {
                        let n = u64_to_str(args[arg_idx], &mut buf[out_pos..]);
                        out_pos += n;
                        arg_idx += 1;
                    }
                }
                b'x' => {
                    if arg_idx < args.len() {
                        let n = u64_to_hex(args[arg_idx], &mut buf[out_pos..]);
                        out_pos += n;
                        arg_idx += 1;
                    }
                }
                b's' => {
                    if arg_idx < args.len() {
                        let s_ptr = args[arg_idx] as *const u8;
                        arg_idx += 1;
                        let s_len = strlen(s_ptr);
                        let copy = s_len.min(buf.len() - out_pos);
                        for j in 0..copy {
                            buf[out_pos + j] = unsafe { s_ptr.add(j).read() };
                        }
                        out_pos += copy;
                    }
                }
                b'%' => {
                    buf[out_pos] = b'%';
                    out_pos += 1;
                }
                _ => {}
            }
            fmt_pos += 1;
        } else {
            buf[out_pos] = fmt[fmt_pos];
            out_pos += 1;
            fmt_pos += 1;
        }
    }

    out_pos
}
