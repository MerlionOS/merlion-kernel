# Cross-Compiling Programs for MerlionOS

## Quick Start: Compile C Programs

```sh
# Install musl cross-compiler
# macOS:
brew install filosottile/musl-cross/musl-cross
# Linux:
sudo apt install musl-tools

# Compile a static C program for MerlionOS
x86_64-linux-musl-gcc -static -nostdlib -ffreestanding \
  -o hello hello.c -Wl,-emain

# Or with our custom target spec:
x86_64-linux-musl-gcc -static -nostdlib \
  -T merlionos.ld -o myprogram myprogram.c
```

## MerlionOS Syscall ABI

```c
// Syscall via int 0x80
// rax = syscall number, rdi = arg1, rsi = arg2, rdx = arg3
// Return value in rax

static long syscall3(long num, long a1, long a2, long a3) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a"(ret)
        : "a"(num), "D"(a1), "S"(a2), "d"(a3)
        : "memory"
    );
    return ret;
}

#define SYS_WRITE    0
#define SYS_EXIT     1
#define SYS_OPEN   100
#define SYS_READ   101
#define SYS_CLOSE  102
// ... see docs/syscall-reference.md for all 115 syscalls
```

## Example: Hello World (C)

```c
// hello.c — runs on MerlionOS in Ring 3
void _start(void) {
    const char *msg = "Hello from C on MerlionOS!\n";
    // SYS_WRITE = 0, arg1 = buf, arg2 = len
    __asm__ volatile (
        "mov $0, %%rax\n"
        "mov %0, %%rdi\n"
        "mov $27, %%rsi\n"
        "int $0x80\n"
        :: "r"(msg) : "rax", "rdi", "rsi"
    );
    // SYS_EXIT = 1, arg1 = 0
    __asm__ volatile (
        "mov $1, %%rax\n"
        "xor %%rdi, %%rdi\n"
        "int $0x80\n"
        ::: "rax", "rdi"
    );
    __builtin_unreachable();
}
```

Compile: `x86_64-linux-musl-gcc -static -nostdlib -o hello hello.c`

## Example: Simple HTTP Proxy (C)

```c
// proxy.c — Envoy-like proxy on MerlionOS
#include "merlion_syscall.h"

#define SYS_SOCKET     130
#define SYS_BIND       134
#define SYS_LISTEN     135
#define SYS_EPOLL_CREATE 230
#define SYS_EPOLL_CTL    231
#define SYS_EPOLL_WAIT   232

void _start(void) {
    // Create listening socket
    long sock = syscall3(SYS_SOCKET, 2, 1, 0);  // AF_INET, SOCK_STREAM

    // Create epoll instance
    long epfd = syscall3(SYS_EPOLL_CREATE, 0, 0, 0);

    // Register socket with epoll
    syscall3(SYS_EPOLL_CTL, (epfd << 32) | 1, sock, 1);  // EPOLL_CTL_ADD, EPOLLIN

    // Event loop
    while (1) {
        long n = syscall3(SYS_EPOLL_WAIT, epfd, 32, -1);
        // Process events...
    }
}
```

## Loading Programs into MerlionOS

### Method 1: VFS (in-kernel)
```sh
# In MerlionOS shell:
# Write program bytes to /bin/myprogram via write command
# Then: run-user myprogram
```

### Method 2: Self-Host Compile
```sh
# Write Rust source to /src/myprogram.rs
# In MerlionOS shell:
compile myprogram
run-user myprogram
```

### Method 3: Embed at Build Time
Add program to `src/userspace.rs::get_builtin_program()`.

## Envoy on MerlionOS

Instead of cross-compiling the full Envoy (1.5M lines C++), MerlionOS provides
**MerlionProxy** — an Envoy-equivalent L7 proxy built into the kernel.

```sh
# In MerlionOS shell:
proxy cluster backend 10.0.0.1:8080
proxy cluster backend2 10.0.0.2:8080
proxy route / backend
proxy route /api backend2
proxy start
proxy status
proxy test /api/v1/users
```

Features matching Envoy:
- L7 HTTP/gRPC routing
- Round-robin / weighted / least-connections load balancing
- Active health checking
- Circuit breaker (closed → open → half-open)
- Retry policies (configurable status codes + backoff)
- Rate limiting (via ratelimit.rs)
- mTLS (via tls.rs)
- Access logging (via http_middleware.rs)
- iptables/NAT integration
- eBPF packet filtering
