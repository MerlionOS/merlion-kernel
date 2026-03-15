# MerlionOS

A Singapore-inspired hobby operating system written in Rust for x86_64.

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

## Current Status (Phase 20)

- 28 source modules, ~4500 lines of Rust
- 45+ shell commands, environment variables, aliases
- Preemptive multitasking, user-mode, VFS, IPC, networking
- See [docs/architecture.md](docs/architecture.md) for design details

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
| 21 | Loadable kernel modules (dynamic driver registration) | Planned |
| 22 | Demand paging + page fault-driven lazy allocation | Planned |
| 23 | Copy-on-write fork (process creation) | Planned |
| 24 | Kernel symbol table + stack trace on panic | Planned |
| 25 | Slab allocator for fixed-size kernel objects | Planned |

### Real Hardware (Phases 26-30)

| Phase | Focus | Status |
|-------|-------|--------|
| 26 | Virtio-blk driver (real block I/O through QEMU) | Planned |
| 27 | FAT16 filesystem on virtio-blk disk | Planned |
| 28 | Virtio-net driver (real Ethernet frames) | Planned |
| 29 | ARP + ICMP (ping) over virtio-net | Planned |
| 30 | TCP/IP stack (connect, listen, accept) | Planned |

### User Space (Phases 31-35)

| Phase | Focus | Status |
|-------|-------|--------|
| 31 | ELF binary loader | Planned |
| 32 | Separate user-space crate (merlion-user) | Planned |
| 33 | POSIX-like syscalls (open, read, write, close, dup) | Planned |
| 34 | User-space libc (memory allocator, printf, string) | Planned |
| 35 | Init process + login shell | Planned |

### SMP & Advanced (Phases 36-40)

| Phase | Focus | Status |
|-------|-------|--------|
| 36 | AP boot (start secondary CPUs via SIPI) | Planned |
| 37 | Per-CPU run queues + load balancing | Planned |
| 38 | Spinlock → ticket lock → MCS lock progression | Planned |
| 39 | APIC timer (replace PIT, per-CPU scheduling) | Planned |
| 40 | IOAPIC + MSI interrupt routing | Planned |
