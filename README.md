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

## Current Status (AI Native OS — Phase G Complete)

- **46 source modules, ~7400 lines of Rust**
- **All 40 kernel phases + 7 AI phases complete**
- **75+ shell commands**
- Monolithic AI-native kernel: NL shell, LLM proxy, semantic VFS,
  AI monitor, self-healing, agent framework, concept explainer

## Roadmap

### Foundation (Phases 1-10)

| Phase | Focus | Status |
|-------|-------|--------|
| 1  | Boot in QEMU, VGA hello, panic handler | Done |
| 2  | Serial logging, GDT/IDT, PIT timer, exceptions | Done |
| 3  | Keyboard input, heap allocator, frame allocator | Done |
| 4  | VGA console + scrolling, shell, `println!` | Done |
| 5  | Uptime, kernel log, page fault handler, user-mode groundwork | Done |
| 6  | Preemptive multitasking, context switching, scheduler | Done |
| 7  | Per-process page tables, syscall ABI, user programs | Done |
| 8  | IPC channels, concurrent processes, expanded syscalls | Done |
| 9  | Virtual filesystem (/dev, /proc, /tmp), task kill | Done |
| 10 | ACPI power management, ANSI colors, driver framework | Done |

### Features (Phases 11-20)

| Phase | Focus | Status |
|-------|-------|--------|
| 11 | RTC clock, kernel self-tests, lib.rs refactor | Done |
| 12 | Framebuffer graphics, basic drawing primitives | Done |
| 13 | PCI bus scan, RAM disk, block filesystem | Done |
| 14 | Networking stack: IPv4, UDP, loopback | Done |
| 15 | SMP: CPUID, APIC, per-CPU state, `cpuinfo` | Done |
| 16 | User-space syscall library (ulib) | Done |
| 17 | Shell: history, arrow keys, shift, echo | Done |
| 18 | Environment variables, aliases, $VAR expansion | Done |
| 19 | neofetch, uname, whoami, hostname | Done |
| 20 | free, sleep, polished shell experience | Done |

### Kernel Evolution (Phases 21-25)

| Phase | Focus | Status |
|-------|-------|--------|
| 21 | Loadable kernel modules (dynamic driver registration) | Done |
| 22 | Demand paging + page fault-driven lazy allocation | Done |
| 23 | Copy-on-write fork (process creation) | Deferred |
| 24 | Kernel symbol table + stack trace on panic | Done |
| 25 | Slab allocator for fixed-size kernel objects | Done |

### Real Hardware (Phases 26-30)

| Phase | Focus | Status |
|-------|-------|--------|
| 26 | Virtio device discovery + virtqueue types | Done |
| 27 | Block device abstraction layer | Done |
| 28 | FAT16-like filesystem (MF16) with cluster chains | Done |
| 29 | ARP table + ICMP ping simulation | Done |
| 30 | TCP state machine types (groundwork) | Done |

### User Space (Phases 31-35)

| Phase | Focus | Status |
|-------|-------|--------|
| 31 | ELF types + user-space library (ulib) | Done |
| 32 | File descriptor table (open, read, write, close) | Done |
| 33 | POSIX-like fd commands (lsof, open, close) | Done |
| 34 | stdin/stdout/stderr initialization | Done |
| 35 | Block device + FAT16 filesystem integration | Done |

### SMP & Advanced (Phases 36-40)

| Phase | Focus | Status |
|-------|-------|--------|
| 36 | AP boot types + SIPI IPI command format | Done |
| 37 | Per-CPU state tracking (up to 16 CPUs) | Done |
| 38 | Spinlock + ticket lock with statistics | Done |
| 39 | APIC timer calibration + register definitions | Done |
| 40 | Lock demo (`lockdemo`) + final integration | Done |
