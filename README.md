# MerlionOS

A Singapore-inspired hobby operating system written in Rust for x86_64.

## Prerequisites

- **Rust nightly** (managed automatically via `rust-toolchain.toml`)
- **rust-src** component: `rustup component add rust-src --toolchain nightly`
- **llvm-tools** component: `rustup component add llvm-tools --toolchain nightly`
- **cargo-bootimage**: `cargo install bootimage`
- **QEMU**: `brew install qemu` (macOS) or `apt install qemu-system-x86` (Linux)

## Build

```sh
make build
```

## Run

```sh
make run
```

Opens a QEMU window with the VGA console. Serial output goes to your terminal. Type commands in the QEMU window.

Headless (serial only):

```sh
make run-serial
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
| `run <prog>`| Run a user-mode program |
| `progs`     | List available user programs |
| `dmesg`     | Kernel log ring buffer |
| `clear`     | Clear screen |
| `umode`     | Test ring 3 transition |
| `panic`     | Trigger a test kernel panic |

## User Programs

| Name      | Description |
|-----------|-------------|
| `hello`   | Prints "Hello userspace!" via sys_write, then exits |
| `counter` | Writes "counting" 3 times with sys_yield between each |

## Project Structure

```
merlion-kernel/
├── src/
│   ├── main.rs          # Kernel entry point and panic handler
│   ├── vga.rs           # VGA text console with scrolling
│   ├── serial.rs        # UART serial port driver (COM1)
│   ├── gdt.rs           # GDT with kernel + user segments, TSS
│   ├── interrupts.rs    # IDT: exceptions, IRQs, raw syscall handler
│   ├── keyboard.rs      # PS/2 scancode set 1 decoder
│   ├── memory.rs        # Page tables, global frame allocator
│   ├── allocator.rs     # Kernel heap allocator
│   ├── timer.rs         # PIT tick counter and uptime
│   ├── log.rs           # Kernel log ring buffer (dmesg)
│   ├── task.rs          # Kernel task management + context switching
│   ├── syscall.rs       # Syscall dispatch (write, exit, yield)
│   ├── process.rs       # User process: page tables, program loading
│   ├── usermode.rs      # Ring 3 transition via iretq
│   └── shell.rs         # Interactive kernel shell
├── .cargo/config.toml
├── rust-toolchain.toml
├── Cargo.toml
├── Makefile
└── README.md
```

## Current Status (Phase 7)

- Per-process page tables (clone kernel upper-half, map user lower-half)
- Syscall ABI via raw int 0x80 handler (rax=num, rdi/rsi/rdx=args)
- sys_write, sys_exit, sys_yield syscalls
- Embedded user programs (hand-assembled x86_64 machine code)
- User code mapped at 0x400000, user stack at 0x800000
- CR3 switch to user page table before ring 3 entry
- Global frame allocator accessible from all subsystems

## Next Milestone (Phase 8)

- Separate user-mode binary build (ELF loader)
- Process isolation and cleanup (free page tables on exit)
- Multiple concurrent user processes
- IPC (inter-process communication) primitives
- Virtual filesystem (VFS) interface
