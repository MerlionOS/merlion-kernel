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

| Command  | Description |
|----------|-------------|
| `help`   | List available commands |
| `info`   | System information |
| `uptime` | Time since boot |
| `heap`   | Heap allocator statistics |
| `ps`     | List running tasks |
| `spawn`  | Spawn a demo task |
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
│   ├── vga.rs               # VGA text console with scrolling
│   ├── serial.rs            # UART serial port driver (COM1)
│   ├── gdt.rs               # GDT with kernel + user segments, TSS
│   ├── interrupts.rs        # IDT: exceptions, IRQs, syscall
│   ├── keyboard.rs          # PS/2 scancode set 1 decoder
│   ├── memory.rs            # Page table access + frame allocator
│   ├── allocator.rs         # Kernel heap allocator
│   ├── timer.rs             # PIT tick counter and uptime
│   ├── log.rs               # Kernel log ring buffer (dmesg)
│   ├── task.rs              # Task management + context switching
│   ├── usermode.rs          # Ring 3 transition via iretq
│   └── shell.rs             # Interactive kernel shell
└── README.md
```

## Current Status (Phase 6)

- Kernel task (thread) management with PID tracking
- Cooperative and preemptive context switching
- Round-robin scheduler triggered by timer interrupt
- Naked function context switch (callee-saved registers + RSP)
- Per-task heap-allocated stacks (16K each)
- `spawn` / `ps` shell commands
- Demo tasks that print, yield, and exit

## Next Milestone (Phase 7)

- Per-process page tables (address space isolation)
- Minimal ELF binary loader
- Expanded syscall interface (write, exit, yield)
- Process lifecycle (fork-like spawning)
