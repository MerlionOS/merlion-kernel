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

Or directly:

```sh
cargo bootimage
```

The bootable image is created at:
`target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin`

## Run

```sh
make run
```

This opens a QEMU window with the VGA boot banner. Serial output (kernel logs) is printed to your terminal.

To run headless (serial only, no GUI window):

```sh
make run-serial
```

## Project Structure

```
merlion-kernel/
├── .cargo/config.toml      # Build target and runner config
├── rust-toolchain.toml      # Pins nightly toolchain
├── Cargo.toml               # Package manifest
├── Makefile                  # Build/run shortcuts
├── src/
│   ├── main.rs              # Kernel entry point and panic handler
│   ├── vga.rs               # VGA text mode buffer writer
│   ├── serial.rs            # UART serial port driver (COM1)
│   ├── gdt.rs               # Global Descriptor Table + TSS
│   └── interrupts.rs        # IDT, exception and interrupt handlers
└── README.md
```

## Current Status (Phase 2)

- Boots in QEMU via `bootloader` crate
- Reaches Rust kernel entry point
- Prints "Hello from MerlionOS!" to VGA text buffer
- Serial port logging (COM1/UART) with `serial_println!` macro
- Panic handler outputs to both serial (with location) and VGA (red text)
- GDT with TSS (separate double fault stack)
- IDT with breakpoint and double fault exception handlers
- PIC initialization and PIT timer interrupt handling
- CPU halts cleanly after boot

## Next Milestone (Phase 3)

- Keyboard input (PS/2 via IRQ1)
- Basic heap allocator
- Physical memory management (frame allocator)
- Page table setup / virtual memory
