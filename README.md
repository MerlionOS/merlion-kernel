# MerlionOS

> **Born for AI. Built by AI.** — 生于AI，成于AI

![Lines of Code](https://img.shields.io/badge/lines-86K+-blue)
![Version](https://img.shields.io/badge/version-v77-green)
![License](https://img.shields.io/badge/license-MIT-orange)
![Rust](https://img.shields.io/badge/rust-nightly-red)
![Architectures](https://img.shields.io/badge/arch-x86__64%20|%20aarch64%20|%20riscv64%20|%20loongarch64-purple)

A Singapore-inspired AI-native operating system kernel written entirely in Rust, targeting four CPU architectures: x86_64, aarch64, RISC-V, and LoongArch. MerlionOS is a from-scratch hobby OS that boots in QEMU with a full interactive shell, preemptive multitasking, networking, graphics, and built-in AI capabilities — all developed in partnership with Claude.

**253 modules | 85,928 lines of Rust | 358 shell commands | 77 releases | 4 architectures**

---

## Feature Highlights

### Core Kernel
- Preemptive multitasking with per-process page tables and ring 3 user mode
- GDT/IDT, PIT timer, APIC, SMP detection, slab allocator
- Demand paging, stack guard canaries, heap integrity checks
- Signal framework (SIGKILL, SIGTERM, SIGSTOP)
- Loadable kernel modules, kernel symbol table, backtraces

### Storage & Filesystems
- In-memory VFS with /dev, /proc, /etc, /tmp
- Virtio-blk and AHCI/NVMe drivers, GPT partition support
- FAT16-like filesystem (MF16), RAM disk, disk-backed FS
- File descriptor table (open/read/write/close), ELF-64 loader

### Networking
- Virtio-net and E1000e drivers, DHCP client
- IPv4/UDP/TCP stack with 3-way handshake
- ARP, ICMP ping, HTTP client (wget), netstat

### Shell & User Space
- 298 commands: process mgmt, file ops, networking, system tools
- Command history, arrow keys, aliases, env variables, scripting
- Interactive apps: text editor, calculator, snake, Forth interpreter
- Screen saver, top, watch, fortune, benchmarks

### AI Native
- Natural language shell (English + Chinese)
- LLM proxy over COM2 serial, semantic VFS with file tags
- AI system monitor, self-healing kernel (page fault / OOM recovery)
- Agent framework, AI-powered man pages, kernel concept explainer

### Graphics & I/O
- VGA text console with ANSI colors, framebuffer (160x50)
- PS/2 keyboard, UART serial, audio engine
- ACPI shutdown/reboot, power management

---

## Supported Architectures

| Architecture | Target | Hardware | Build | Test |
|---|---|---|---|---|
| x86_64 | Intel/AMD PC | BIOS + UEFI | `make build` / `make iso` | `make run` / `make run-uefi-mac` |
| aarch64 | Raspberry Pi 3/4/5 | Pi firmware | `make pi` | `make run-pi` |
| riscv64 | RISC-V (SiFive, StarFive) | OpenSBI | `make riscv` | `make run-riscv` |
| loongarch64 | Loongson 3A5000/6000 | UEFI | `make loongarch` | `make run-loongarch` |

---

## Quick Start

**Prerequisites:** Rust nightly, QEMU

```sh
rustup component add rust-src llvm-tools --toolchain nightly
cargo install bootimage
```

**Build and run (x86_64 default):**

```sh
make build       # build bootable image
make run         # boot in QEMU (VGA + serial)
make run-serial  # headless (serial only)
```

**Other architectures:**

```sh
make pi          # build for Raspberry Pi (aarch64)
make run-pi      # boot aarch64 in QEMU
make riscv       # build for RISC-V
make run-riscv   # boot riscv64 in QEMU
make loongarch   # build for LoongArch
make run-loongarch  # boot loongarch64 in QEMU
```

---

## Architecture

MerlionOS supports four CPU architectures with a shared kernel core and architecture-specific HAL layers. The primary target is `x86_64-unknown-none` using the `bootloader` crate v0.9 with physical memory mapping. The kernel boots with a GDT (kernel + user segments with TSS), sets up an IDT for exceptions and PIC IRQs, initializes a 4MB heap via a linked-list allocator, and starts a preemptive round-robin scheduler driven by the PIT at 100 Hz. User processes run in ring 3 with cloned page tables (kernel in upper half, user in lower half) and communicate via syscalls through `int 0x80`. The VFS, networking stack, and AI subsystems are all in-kernel modules initialized at boot. Each architecture port provides its own interrupt controller, timer, UART, and boot sequence while sharing the platform-independent kernel subsystems.

---

## Links

- **Website:** [merlionos.org](https://merlionos.org)
- **Docs:** [Architecture](docs/architecture.md) | [AI Native OS](docs/ai-native-os.md) | [Deep Dive](docs/deep-dive-roadmap.md)
- **中文文档:** [README_CN.md](README_CN.md)
- **License:** [MIT](LICENSE)

---

*Built with AI — MerlionOS is developed in partnership with [Claude](https://claude.ai) by Anthropic, from architecture design to implementation. Every line of code is a collaboration between human vision and AI capability.*
