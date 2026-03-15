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

Or directly:

```sh
qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin
```

A QEMU window will open showing the MerlionOS boot banner.

## Project Structure

```
merlion-kernel/
├── .cargo/config.toml      # Build target and runner config
├── rust-toolchain.toml      # Pins nightly toolchain
├── Cargo.toml               # Package manifest
├── Makefile                  # Build/run shortcuts
├── src/
│   ├── main.rs              # Kernel entry point and panic handler
│   └── vga.rs               # VGA text mode buffer writer
└── README.md
```

## Current Status (Phase 1)

- Boots in QEMU via `bootloader` crate
- Reaches Rust kernel entry point
- Prints "Hello from MerlionOS!" to VGA text buffer
- Panic handler writes to screen in red
- CPU halts cleanly after boot

## Next Milestone (Phase 2)

- Serial port (UART) logging
- Improved panic output with location info
- GDT (Global Descriptor Table) setup
- IDT (Interrupt Descriptor Table) setup
- Basic exception handlers (double fault, etc.)
- Timer interrupt (PIT) groundwork
