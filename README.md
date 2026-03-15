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

Opens a QEMU window with the VGA console. Serial log output goes to your terminal. Type commands in the QEMU window.

Headless (serial only):

```sh
make run-serial
```

## Shell Commands

| Command  | Description |
|----------|-------------|
| `help`   | List available commands |
| `info`   | System information |
| `uptime` | Time since boot |
| `heap`   | Heap allocator statistics |
| `dmesg`  | Kernel log ring buffer |
| `clear`  | Clear screen |
| `umode`  | Test user-mode (ring 3) transition |
| `panic`  | Trigger a test kernel panic |

## Project Structure

```
merlion-kernel/
├── .cargo/config.toml      # Build target and runner config
├── rust-toolchain.toml      # Pins nightly toolchain
├── Cargo.toml               # Package manifest
├── Makefile                  # Build/run shortcuts
├── src/
│   ├── main.rs              # Kernel entry point and panic handler
│   ├── vga.rs               # VGA text console with scrolling and cursor
│   ├── serial.rs            # UART serial port driver (COM1)
│   ├── gdt.rs               # GDT with kernel + user segments, TSS
│   ├── interrupts.rs        # IDT: exceptions, hardware IRQs, syscall
│   ├── keyboard.rs          # PS/2 scancode set 1 decoder
│   ├── memory.rs            # Page table access + frame allocator
│   ├── allocator.rs         # Kernel heap allocator
│   ├── timer.rs             # PIT tick counter and uptime tracking
│   ├── log.rs               # Kernel log ring buffer (dmesg)
│   ├── usermode.rs          # Ring 3 transition via iretq + int 0x80
│   └── shell.rs             # Interactive kernel shell
└── README.md
```

## Current Status (Phase 5)

- PIT timer at 100 Hz with uptime tracking
- Kernel log ring buffer (4K) with `dmesg` command
- Page fault handler with diagnostic output
- GDT user-mode segments (ring 3 code/data)
- TSS kernel stack for privilege transitions
- Syscall handler (int 0x80) callable from ring 3
- User-mode proof-of-concept via iretq

## Next Milestone (Phase 6)

- Process abstraction (PCB, PID)
- Per-process page tables
- ELF binary loader (minimal)
- Context switching between kernel tasks
- Syscall interface expansion (write, exit)
