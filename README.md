# MerlionOS

[![Build & Boot Test](https://github.com/MerlionOS/merlion-kernel/actions/workflows/build.yml/badge.svg)](https://github.com/MerlionOS/merlion-kernel/actions/workflows/build.yml)

> **Born for AI. Built by AI.** — 生于AI，成于AI

[中文文档](README_CN.md) | [Architecture](docs/architecture.md) | [AI Native OS](docs/ai-native-os.md) | [Deep Dive](docs/deep-dive-roadmap.md)

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

## Current Status

- **58 source modules, ~9700 lines of Rust**
- **All roadmap phases complete + 3 deep-dive phases**
- **90+ shell commands**, `demo` runs full showcase
- Real virtio-blk disk I/O, ELF loading, virtio-net with ARP/ICMP

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
| 56 | ELF-64 binary parser (header, program headers) | Done |
| 57 | User-space libc: memset, memcpy, strlen, snprintf | Done |
| 58 | readelf command + kernel binary info | Done |
| 59 | u64/hex formatting for user-space programs | Done |
| 60 | User-space library integration (ulib + userlib) | Done |

### 12. AI Integration (Phases 61-65) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 61 | AI-powered `man` pages for all commands | Done |
| 62 | AI classify + auto-tag in syscall layer | Done |
| 63 | AI kernel concept explainer (7 topics) | Done |
| 64 | `man <cmd>` with formatted output | Done |
| 65 | Conversational agents (greeter, health, explain) | Done |

### 13. Beyond (Phases 66-70) — Planned

| Phase | Focus | Status |
|-------|-------|--------|
| 66 | Boot info system (method, arch, bootloader detection) | Done |
| 67 | Architecture abstraction (x86_64 + aarch64 types) | Done |
| 68 | Framebuffer 160×50 with drawing primitives | Done |
| 69 | `bootinfo` command + system topology display | Done |
| 70 | Final integration — all 70 phases complete | Done |
