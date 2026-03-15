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

This opens a QEMU window with the VGA boot banner. Serial output (kernel logs) is printed to your terminal. Keyboard input in the QEMU window is echoed to serial.

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
│   ├── interrupts.rs        # IDT, exception and interrupt handlers
│   ├── keyboard.rs          # PS/2 scancode set 1 decoder
│   ├── memory.rs            # Page table access + frame allocator
│   └── allocator.rs         # Kernel heap allocator
└── README.md
```

## Current Status (Phase 3)

- Boots in QEMU via `bootloader` crate
- VGA text mode banner and serial logging
- GDT/TSS, IDT with exception handlers (breakpoint, double fault)
- PIC + PIT timer interrupt + PS/2 keyboard input (IRQ1)
- Physical memory frame allocator (from bootloader memory map)
- Page table access via bootloader's physical memory mapping
- 64K kernel heap with linked-list allocator
- Panic handler outputs to both serial and VGA

## Next Milestone (Phase 4)

- VGA text mode scrolling and cursor
- Shell-like command input over serial or VGA
- Improved frame allocator (bitmap or buddy)
- User-mode groundwork (ring 3 transition)
