# MerlionOS

A Singapore-inspired hobby operating system written in Rust for x86_64.

## Prerequisites

- **Rust nightly** (managed automatically via `rust-toolchain.toml`)
- **rust-src** component: `rustup component add rust-src --toolchain nightly`
- **llvm-tools** component: `rustup component add llvm-tools --toolchain nightly`
- **cargo-bootimage**: `cargo install bootimage`
- **QEMU**: `brew install qemu` (macOS) or `apt install qemu-system-x86` (Linux)

## Build & Run

```sh
make build     # build bootable image
make run       # boot in QEMU (VGA + serial)
make run-serial # headless (serial only)
```

## Shell Commands

| Command     | Description |
|-------------|-------------|
| `help`      | List available commands |
| `info`      | System information |
| `uptime`    | Time since boot |
| `heap`      | Heap allocator statistics |
| `ps`        | List running tasks |
| `spawn`     | Spawn a demo kernel task |
| `run <prog>`| Run user program (blocking) |
| `bg <prog>` | Run user program (background) |
| `progs`     | List available user programs |
| `pipe`      | IPC demo (producer/consumer) |
| `channels`  | List active IPC channels |
| `dmesg`     | Kernel log ring buffer |
| `clear`     | Clear screen |
| `panic`     | Trigger a test kernel panic |

## Syscall ABI (int 0x80)

| # | Name    | Args | Description |
|---|---------|------|-------------|
| 0 | write   | rdi=buf, rsi=len | Print to serial+VGA |
| 1 | exit    | rdi=code | Terminate process |
| 2 | yield   | — | Yield to scheduler |
| 3 | getpid  | — | Get current PID |
| 4 | sleep   | rdi=ticks | Sleep for N ticks |
| 5 | send    | rdi=chan, rsi=byte | Send to IPC channel |
| 6 | recv    | rdi=chan | Receive from IPC channel |

## Project Structure

```
src/
├── main.rs          # Kernel entry point
├── vga.rs           # VGA text console with scrolling
├── serial.rs        # UART serial driver (COM1)
├── gdt.rs           # GDT with kernel + user segments, TSS
├── interrupts.rs    # IDT: exceptions, IRQs, raw syscall trampoline
├── keyboard.rs      # PS/2 scancode decoder
├── memory.rs        # Page tables, global frame allocator
├── allocator.rs     # Kernel heap allocator
├── timer.rs         # PIT tick counter and uptime
├── log.rs           # Kernel log ring buffer
├── task.rs          # Task management + context switching
├── syscall.rs       # Syscall dispatch (7 syscalls)
├── process.rs       # User processes: page tables, program loading
├── ipc.rs           # Inter-process communication channels
└── shell.rs         # Interactive kernel shell
```

## Current Status (Phase 8)

- Concurrent user processes (background execution via `bg`)
- Process frame tracking and cleanup on exit
- 7 syscalls: write, exit, yield, getpid, sleep, send, recv
- IPC bounded channels (64-byte ring buffers)
- Producer/consumer demo (`pipe` command)
- Removed legacy usermode.rs in favor of process.rs

## Next Milestone (Phase 9)

- ELF binary loader (load from embedded or in-memory images)
- Virtual filesystem (VFS) with /dev/serial, /proc/*
- Signal handling (SIGKILL for `kill` command)
- Kernel module / driver interface
