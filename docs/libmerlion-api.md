# libmerlion — Userspace Library API

The MerlionOS userspace library provides C-standard functions running in Ring 3.
Functions are loaded at `LIBC_BASE` (0x0050_0000) as x86_64 machine code.

## Calling Convention

All functions use the System V AMD64 ABI:
- Arguments: `rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9`
- Return value: `rax`
- Caller-saved: `rax`, `rcx`, `rdx`, `rsi`, `rdi`, `r8-r11`

Call via: `movabs rax, <func_addr>; call rax`

## I/O Functions

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `write` | 0x500000 | `write(buf, len) → n` | Write buffer to stdout |
| `exit` | 0x500010 | `exit(code) → !` | Terminate process |
| `puts` | 0x500238 | `puts(str) → n` | Write null-terminated string |
| `print_int` | 0x500270 | `print_int(num)` | Print integer to stdout |
| `printf` | 0x500220 | `printf(fmt, len, arg) → n` | Formatted output (%d/%x/%s) |

## String Functions

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `strlen` | 0x500020 | `strlen(str) → len` | Length of null-terminated string |
| `strcmp` | 0x500070 | `strcmp(s1, s2) → result` | Compare strings (0=equal) |
| `memcpy` | 0x500040 | `memcpy(dst, src, len) → dst` | Copy memory (rep movsb) |
| `memset` | 0x500058 | `memset(ptr, val, len) → ptr` | Fill memory (rep stosb) |

## Memory Management

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `malloc` | 0x5000A0 | `malloc(size) → ptr` | Bump allocator via brk (16-byte aligned) |
| `free` | 0x500100 | `free(ptr)` | No-op (reclaimed on exit) |
| `brk` | 0x500148 | `brk(addr) → new_brk` | Set heap break point |

## File I/O

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `open` | 0x500108 | `open(path, len, flags) → fd` | Open file |
| `read` | 0x500118 | `read(fd, buf, len) → n` | Read from fd |
| `close` | 0x500128 | `close(fd) → 0` | Close fd |

## Process

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `getpid` | 0x500138 | `getpid() → pid` | Current process ID |
| `sleep` | 0x500158 | `sleep(ms)` | Sleep milliseconds |
| `gettime` | 0x5001A8 | `gettime() → secs` | Seconds since boot |

## Network

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `socket` | 0x500168 | `socket(dom, type, proto) → fd` | Create socket |
| `connect` | 0x500178 | `connect(fd, addr, len) → 0` | TCP connect |
| `sendto` | 0x500188 | `sendto(fd, buf, len) → n` | Send data |
| `recvfrom` | 0x500198 | `recvfrom(fd, buf, len) → n` | Receive data |

## Utility

| Function | Address | Signature | Description |
|----------|---------|-----------|-------------|
| `itoa` | 0x5001B8 | `itoa(num, buf) → digits` | Integer to ASCII string |

## Example: Rust `no_std` Program (Conceptual)

```rust
#![no_std]
#![no_main]

// Syscall wrappers would look like:
fn write(buf: &[u8]) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "mov rax, 0",    // SYS_WRITE
            "int 0x80",
            in("rdi") buf.as_ptr(),
            in("rsi") buf.len(),
            out("rax") ret,
        );
    }
    ret
}

fn exit(code: i32) -> ! {
    unsafe {
        core::arch::asm!(
            "mov rax, 1",    // SYS_EXIT
            "int 0x80",
            in("rdi") code,
            options(noreturn),
        );
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write(b"Hello from Rust userspace!\n");
    exit(0);
}
```

Compile with: `rustc --target x86_64-unknown-none -C link-arg=-nostartfiles`
Store ELF at `/bin/<name>` in VFS, execute with `run-user <name>`.
