# MerlionOS

> **Born for AI. Built by AI.** — 生于AI，成于AI

[中文文档](README_CN.md)

A Singapore-inspired AI-native hobby operating system written in Rust for x86_64.

## Prerequisites

- **Rust nightly** (managed automatically via `rust-toolchain.toml`)
- **rust-src**: `rustup component add rust-src --toolchain nightly`
- **llvm-tools**: `rustup component add llvm-tools --toolchain nightly`
- **cargo-bootimage**: `cargo install bootimage`
- **QEMU**: `brew install qemu` (macOS) or `apt install qemu-system-x86` (Linux)

## Build & Run

```sh
make build       # build bootable image
make run         # boot in QEMU (VGA + serial)
make run-serial  # headless (serial only)
```

## Shell Commands

### Process Management
| Command      | Description |
|--------------|-------------|
| `ps`         | List running tasks |
| `spawn`      | Spawn a demo kernel task |
| `kill <pid>` | Kill a task by PID |
| `bg <prog>`  | Run user program in background |
| `run <prog>` | Run user program (blocking) |
| `progs`      | List user programs |

### File Operations
| Command              | Description |
|----------------------|-------------|
| `ls [path]`          | List directory (default: /) |
| `cat <path>`         | Read file contents |
| `write <path> <data>`| Write data to file |
| `rm <path>`          | Remove a file |

### System
| Command    | Description |
|------------|-------------|
| `info`     | System information |
| `uptime`   | Time since boot |
| `heap`     | Heap allocator stats |
| `memmap`   | Physical memory map (color-coded) |
| `drivers`  | List kernel drivers |
| `pipe`     | IPC producer/consumer demo |
| `channels` | List IPC channels |
| `dmesg`    | Kernel log buffer |
| `clear`    | Clear screen |
| `shutdown` | Power off (ACPI) |
| `reboot`   | Restart (keyboard controller reset) |
| `panic`    | Trigger test panic |

## Virtual Filesystem

```
/
├── dev/
│   ├── null       # discard sink
│   └── serial     # COM1 serial port
├── proc/
│   ├── uptime     # system uptime
│   ├── meminfo    # heap statistics
│   └── tasks      # running task list
└── tmp/           # writable user files
```

## Syscall ABI (int 0x80)

| # | Name  | Args | Description |
|---|-------|------|-------------|
| 0 | write | rdi=buf, rsi=len | Print to serial+VGA |
| 1 | exit  | rdi=code | Terminate process |
| 2 | yield | — | Yield to scheduler |
| 3 | getpid| — | Get current PID |
| 4 | sleep | rdi=ticks | Sleep for N ticks |
| 5 | send  | rdi=chan, rsi=byte | Send to IPC channel |
| 6 | recv  | rdi=chan | Receive from IPC channel |

## Project Structure

```
src/
├── main.rs          # Kernel entry point
├── acpi.rs          # ACPI shutdown and reboot
├── allocator.rs     # Kernel heap
├── driver.rs        # Kernel driver framework
├── gdt.rs           # GDT + TSS
├── interrupts.rs    # IDT, exceptions, IRQs, syscall
├── ipc.rs           # IPC channels
├── keyboard.rs      # PS/2 scancode decoder
├── log.rs           # Kernel log ring buffer
├── memory.rs        # Page tables, frame allocator, memory map
├── process.rs       # User processes + page tables
├── serial.rs        # UART serial driver
├── shell.rs         # Interactive kernel shell
├── syscall.rs       # Syscall dispatch
├── task.rs          # Task management + context switching
├── timer.rs         # PIT tick counter
├── vfs.rs           # Virtual filesystem
└── vga.rs           # VGA console with ANSI color support
```

## Current Status (Phase 55)

- **52 source modules, ~8200 lines of Rust**
- **55 kernel phases + 7 AI phases complete**
- **85+ shell commands**
- Virtio-blk/net detection, TCP stack, netstat, full networking

## Roadmap

### 1. Foundation (Phases 1-10) ✅

Boot, GDT/IDT, PIT, keyboard, heap, frame allocator, VGA console,
shell, preemptive multitasking, user-mode ring 3, per-process page tables,
syscalls, IPC channels, VFS, ACPI shutdown/reboot, driver framework.

### 2. Features (Phases 11-20) ✅

RTC clock, kernel self-tests, framebuffer graphics (160×50),
PCI bus scan, RAM disk, IPv4/UDP networking, CPUID/SMP detection,
user-space syscall library, command history, arrow keys, shift,
environment variables, aliases, neofetch, free, sleep.

### 3. Kernel Evolution (Phases 21-25) ✅

Loadable kernel modules, demand paging, kernel symbol table + backtrace,
slab allocator (task, ipc_msg, fd, page_info caches).

### 4. Real Hardware (Phases 26-30) ✅

Virtio device discovery, block device abstraction, FAT16-like filesystem (MF16),
ARP table, ICMP ping, TCP state machine types.

### 5. User Space (Phases 31-35) ✅

File descriptor table (open/read/write/close), stdin/stdout/stderr,
user-space syscall library, block device integration.

### 6. SMP & Advanced (Phases 36-40) ✅

CPUID/APIC detection, per-CPU state, spinlock vs ticket lock,
APIC timer calibration, lock demo.

### 7. AI Native OS (Phases A-G) ✅

| Phase | Focus | Status |
|-------|-------|--------|
| A | AI Shell: natural language → command (中英文) | Done |
| B | LLM Proxy: COM2 serial protocol to external LLM | Done |
| C | Semantic VFS: file tags, search by meaning | Done |
| D | AI System Monitor: anomaly detection, health check | Done |
| E | AI Syscalls: infer, classify, auto-tag, explain | Done |
| F | Self-Healing Kernel: diagnose page fault, OOM, auto-recover | Done |
| G | Agent Framework: health, greeter, explain agents | Done |

### 8. Shell & Scripting (Phases 41-42) ✅

Shell scripting (exec), semicolon chaining, wc, AI-enhanced panic diagnosis.

### 9. Hardening & Polish (Phases 43-50) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 43 | Stack guard canaries for overflow detection | Done |
| 44 | Heap integrity checking (bounds, exhaustion) | Done |
| 45 | /proc expansion (version, cpuinfo, modules, self) | Done |
| 46 | VFS /etc directory for config files | Done |
| 47 | Signal framework (SIGKILL, SIGTERM, SIGSTOP) | Done |
| 48 | Shell semicolon chaining (cmd1 ; cmd2) | Done |
| 49 | Shell script execution (exec command) | Done |
| 50 | Kernel config system (/etc/merlion.conf) | Done |

### 10. Real I/O (Phases 51-55) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 51 | Virtio-blk device detection + block device registration | Done |
| 52 | Virtio-net device detection + driver registration | Done |
| 53 | TCP connection state machine (connect/send/recv/close) | Done |
| 54 | TCP 3-way handshake simulation + loopback echo | Done |
| 55 | netstat, tcpconn, tcpsend, tcprecv, tcpclose commands | Done |

### 11. True User Space (Phases 56-60) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 56 | ELF binary parser and loader | Planned |
| 57 | Separate merlion-user crate (cross-compiled) | Planned |
| 58 | User-space libc: malloc, printf, string ops | Planned |
| 59 | Init process + multi-user login | Planned |
| 60 | User-space shell (msh as standalone binary) | Planned |

### 12. AI Integration (Phases 61-65) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 61 | Virtio AI device (custom QEMU device for inference) | Planned |
| 62 | AI-assisted task scheduler (workload prediction) | Planned |
| 63 | Natural language VFS queries ("find large files") | Planned |
| 64 | AI-powered `man` pages (explain any command) | Planned |
| 65 | Conversational system administration agent | Planned |

### 13. Beyond (Phases 66-70) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 66 | UEFI boot (replace BIOS bootloader) | Planned |
| 67 | x86_64 → aarch64 cross-architecture port | Planned |
| 68 | Framebuffer GUI: window manager, mouse | Planned |
| 69 | USB HID driver (keyboard/mouse) | Planned |
| 70 | Self-hosting: compile Rust inside MerlionOS | Planned |
