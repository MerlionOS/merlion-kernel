# CLAUDE.md — MerlionOS Kernel

## Project Overview

MerlionOS is a Singapore-inspired AI-native hobby operating system kernel written in Rust, targeting four CPU architectures: x86_64, aarch64 (Raspberry Pi), riscv64 (RISC-V), and loongarch64 (LoongArch).

> **Born for AI. Built by AI.** — 生于AI，成于AI

It boots in QEMU, runs a shell with 358 commands, and supports preemptive multitasking, user-mode processes with per-process page tables, IPC, VFS, real virtio-blk/net drivers, ELF loading, TCP/IP, framebuffer graphics, AI native features (NL shell, agents, self-healing), and ACPI power management. 253 source modules, 85,928 lines of Rust.

## Architecture Targets

| Architecture | Target Triple | Build | Run |
|---|---|---|---|
| x86_64 | `x86_64-unknown-none` | `make build` / `make iso` | `make run` / `make run-uefi-mac` |
| aarch64 | `aarch64-unknown-none` | `make pi` | `make run-pi` |
| riscv64 | `riscv64gc-unknown-none-elf` | `make riscv` | `make run-riscv` |
| loongarch64 | `loongarch64-unknown-none` | `make loongarch` | `make run-loongarch` |

## Build & Run

```sh
make build       # cargo bootimage
make run         # QEMU with VGA window + serial on terminal
make run-serial  # headless, serial only
```

Requires: Rust nightly (via rust-toolchain.toml), rust-src, llvm-tools, cargo-bootimage, qemu-system-x86_64 (+ qemu-system-aarch64, qemu-system-riscv64, qemu-system-loongarch64 for other architectures).

## Architecture

- **Target**: `x86_64-unknown-none` with `-C relocation-model=static` (required by bootloader 0.9)
- **Boot**: `bootloader` crate v0.9 with `map_physical_memory` feature
- **Entry**: `entry_point!(kernel_main)` macro provides `&'static BootInfo`
- **No std, no main**: `#![no_std]`, `#![no_main]`, `#![feature(abi_x86_interrupt)]`
- **Heap**: `alloc` crate via `build-std`, 64K linked-list allocator at `0x4444_4444_0000`

## Module Map

| Module | Purpose |
|--------|---------|
| `main.rs` | Entry point, panic handler, init sequence |
| `gdt.rs` | GDT with kernel+user segments, TSS with double fault + ring3 stacks |
| `interrupts.rs` | IDT: exceptions, PIC IRQs, raw naked syscall trampoline (int 0x80) |
| `timer.rs` | PIT at 100 Hz, tick counter, uptime |
| `keyboard.rs` | PS/2 scancode set 1 decoder (make codes only, no modifiers) |
| `serial.rs` | UART 16550 on COM1 (0x3F8), `serial_println!` macro |
| `vga.rs` | VGA text mode console with scrolling, cursor, ANSI color escapes |
| `memory.rs` | Global frame allocator (behind Mutex), page table helpers, memory map display |
| `allocator.rs` | Kernel heap via `linked_list_allocator`, mapped pages |
| `task.rs` | Kernel tasks: spawn, yield, exit, kill, preemptive round-robin, naked asm context switch |
| `process.rs` | User processes: per-process page tables (clone kernel PML4 upper half), embedded x86_64 machine code programs, CR3 switch + iretq to ring 3 |
| `syscall.rs` | Syscall dispatch: write, exit, yield, getpid, sleep, send, recv |
| `ipc.rs` | Bounded ring-buffer channels (64 bytes each) |
| `vfs.rs` | In-memory VFS: directories, regular files, /dev/null, /dev/serial, /proc/* |
| `driver.rs` | Driver registration framework |
| `acpi.rs` | Shutdown (port 0x604) and reboot (keyboard controller 0xFE) |
| `shell.rs` | Interactive shell, command dispatch, demo tasks |
| `log.rs` | 4K ring-buffer kernel log, `klog_println!` macro |

## Key Design Decisions

- **Bootloader 0.9** (not 0.11+): simpler API, well-documented, uses `bootimage` tool
- **Static relocation model**: required because bootloader 0.9 can't load PIE binaries
- **User programs as raw machine code**: hand-assembled x86_64 bytes embedded in `process.rs`, avoids need for ELF loader or separate build step
- **Cooperative + preemptive scheduling**: tasks call `yield_now()` explicitly; timer interrupt also triggers `timer_tick()` which yields if other tasks are ready
- **Context switch via naked function**: pushes/pops callee-saved registers (rbx, rbp, r12-r15), swaps RSP via raw pointer
- **Single address space for kernel tasks**: all tasks share kernel page tables; user processes get cloned PML4 with user mappings in lower half
- **Syscall via int 0x80 + naked trampoline**: saves user registers, extracts rax/rdi/rsi/rdx, calls Rust dispatch function, iretq back

## GDT Layout

```
Index 0: null
Index 1: kernel code (selector 0x08)
Index 2: kernel data (selector 0x10)
Index 3-4: TSS (selector 0x18, occupies two slots)
Index 5: user data (selector 0x2B with RPL=3)
Index 6: user code (selector 0x33 with RPL=3)
```

## Conventions

- Log to both serial and VGA for visibility: `serial_println!` + `println!`
- Use `klog_println!` for the ring buffer (viewable via `dmesg`)
- Interrupts disabled during context switch and serial/VGA writes (deadlock prevention)
- Task slot 0 is always the kernel/idle task (pid 0), never killable
- VFS inode 0 is root `/`, parents reference by inode index

## Common Tasks

- **Add a shell command**: add a match arm in `shell.rs::dispatch()`
- **Add a syscall**: add constant + match arm in `syscall.rs`, update the user program machine code in `process.rs` if needed
- **Add a VFS node**: add an inode in `vfs.rs::Filesystem::new()` and handle its `NodeType` in `read_file`/`write_file`
- **Add a driver**: call `driver::register()` in `driver.rs::init()` or from the new driver's init
- **Add a proc file**: add a `NodeType` variant, create the inode in `Filesystem::new()`, handle read in `read_file()`

## Testing

No test framework yet. Verify manually:
```sh
make run-serial   # boot headless, check serial output for [ok] lines
# In QEMU window: type 'info', 'ps', 'cat /proc/uptime', 'spawn', etc.
```

QEMU exits cleanly on `shutdown` command or timeout (exit code 124 from gtimeout is normal).
