# MerlionOS

> **Born for AI. Built by AI.** — 生于AI，成于AI

![Lines of Code](https://img.shields.io/badge/lines-65K+-blue)
![Version](https://img.shields.io/badge/version-v60-green)
![License](https://img.shields.io/badge/license-MIT-orange)
![Rust](https://img.shields.io/badge/rust-nightly-red)

A Singapore-inspired AI-native operating system kernel written entirely in Rust for x86_64. MerlionOS is a from-scratch hobby OS that boots in QEMU with a full interactive shell, preemptive multitasking, networking, graphics, and built-in AI capabilities — all developed in partnership with Claude.

**223 modules | 65K+ lines of Rust | 298 shell commands | 60 releases**

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

## Quick Start

**Prerequisites:** Rust nightly, QEMU

```sh
rustup component add rust-src llvm-tools --toolchain nightly
cargo install bootimage
```

**Build and run:**

```sh
make build       # build bootable image
make run         # boot in QEMU (VGA + serial)
make run-serial  # headless (serial only)
```

---

## Architecture

MerlionOS targets `x86_64-unknown-none` using the `bootloader` crate v0.9 with physical memory mapping. The kernel boots with a GDT (kernel + user segments with TSS), sets up an IDT for exceptions and PIC IRQs, initializes a 64K heap via a linked-list allocator, and starts a preemptive round-robin scheduler driven by the PIT at 100 Hz. User processes run in ring 3 with cloned page tables (kernel in upper half, user in lower half) and communicate via syscalls through `int 0x80`. The VFS, networking stack, and AI subsystems are all in-kernel modules initialized at boot.

---

## Links

- **Website:** [merlionos.org](https://merlionos.org)
- **Docs:** [Architecture](docs/architecture.md) | [AI Native OS](docs/ai-native-os.md) | [Deep Dive](docs/deep-dive-roadmap.md)
- **中文文档:** [README_CN.md](README_CN.md)
- **License:** [MIT](LICENSE)

---

*Built with AI — MerlionOS is developed in partnership with [Claude](https://claude.ai) by Anthropic, from architecture design to implementation. Every line of code is a collaboration between human vision and AI capability.*
