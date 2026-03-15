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
| `pipe`     | IPC producer/consumer demo |
| `channels` | List IPC channels |
| `dmesg`    | Kernel log buffer |
| `clear`    | Clear screen |
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
├── vga.rs           # VGA text console
├── serial.rs        # UART serial driver
├── gdt.rs           # GDT + TSS
├── interrupts.rs    # IDT, exceptions, IRQs, syscall trampoline
├── keyboard.rs      # PS/2 scancode decoder
├── memory.rs        # Page tables, global frame allocator
├── allocator.rs     # Kernel heap
├── timer.rs         # PIT tick counter
├── log.rs           # Kernel log ring buffer
├── task.rs          # Task management + context switching
├── syscall.rs       # Syscall dispatch (7 syscalls)
├── process.rs       # User processes + page tables
├── ipc.rs           # Inter-process communication channels
├── vfs.rs           # Virtual filesystem
└── shell.rs         # Interactive kernel shell
```

## Current Status (Phase 9)

- Virtual filesystem with /dev, /proc, and /tmp
- Proc files: uptime, meminfo, tasks (generated dynamically)
- Device nodes: /dev/null, /dev/serial
- File operations: ls, cat, write, rm
- Task kill by PID
- 16 source files, ~1800 lines of Rust

## Next Milestone (Phase 10)

- ELF binary loader
- Kernel module / driver interface
- ACPI/shutdown support
- Improved VGA console (colors, escape sequences)
