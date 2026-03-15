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

This opens a QEMU window with the VGA console. Serial log output is printed to your terminal. Type commands in the QEMU window.

To run headless (serial only, no GUI window):

```sh
make run-serial
```

## Shell Commands

| Command | Description |
|---------|-------------|
| `help`  | List available commands |
| `info`  | Show system information |
| `clear` | Clear the screen |
| `heap`  | Show heap allocator statistics |
| `panic` | Trigger a test kernel panic |

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
│   ├── gdt.rs               # Global Descriptor Table + TSS
│   ├── interrupts.rs        # IDT, exception and interrupt handlers
│   ├── keyboard.rs          # PS/2 scancode set 1 decoder
│   ├── memory.rs            # Page table access + frame allocator
│   ├── allocator.rs         # Kernel heap allocator
│   └── shell.rs             # Interactive kernel shell
└── README.md
```

## Current Status (Phase 4)

- VGA text console with scrolling, cursor, `println!` macro
- Interactive shell with command dispatch
- O(1) physical frame allocator
- PS/2 keyboard input routed to shell
- Heap statistics reporting
- Boot log visible on both VGA and serial

## Next Milestone (Phase 5)

- Uptime tracking via PIT tick counter
- Kernel log ring buffer (in-memory `dmesg`)
- Page fault handler
- User-mode groundwork (ring 3 transition)
