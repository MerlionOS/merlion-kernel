# MerlionOS: 132,670 Lines of Rust — From Zero to Self-Hosting OS, Built Entirely with AI

I built an operating system from scratch using AI (Claude). 100 versions, 132,670 lines of Rust, and it can now compile itself.

## What is it?

MerlionOS is a hobby OS kernel written entirely in Rust for x86_64, with ports to ARM (Raspberry Pi), RISC-V, and LoongArch. It boots on real hardware via UEFI.

## Stats
- 132,670 lines of Rust, ~360 modules
- 4 CPU architectures
- 480+ shell commands
- UEFI boot on real hardware
- 0 compiler errors, 0 warnings
- 100% built with AI (Claude Opus)

## What can it do?

**Networking**: Full TCP/IP stack with HTTP/1.1+2+3 (QUIC), gRPC, FTP, SSH, SMTP/IMAP, MQTT, WebSocket, DNS, DHCP server, WireGuard VPN, iptables/NAT, OSPF/BGP routing, eBPF/XDP, DPDK — basically a Linux networking stack in miniature.

**Desktop**: Window compositor with virtual desktops, taskbar, Alt+Tab, file manager, settings app, notification system.

**Applications**: Web browser (HTML/CSS), email client, music player, vim editor, games (Snake, Tetris).

**AI**: Runs LLM inference in-kernel using INT4/INT8 quantization (no floats, no GPU). AI-powered system administration that monitors, diagnoses, and auto-tunes the OS.

**The Ultimate Feature**: A Rust subset compiler + x86_64 assembler + ELF linker — the OS can compile programs and (conceptually) itself.

## How was it built?

The entire OS was built through conversation with Claude (Anthropic's AI). I described what I wanted, Claude generated the code, I tested it, and we iterated. Most work was done using parallel AI agents — sometimes 5-7 agents building different modules simultaneously.

A typical workflow:
1. "Add WiFi 802.11 support" → Agent writes ~1000 lines
2. "Add OSPF routing protocol" → Another agent writes ~700 lines
3. Both run in parallel, I merge and test
4. Fix any build errors, commit, push

The entire v27-v100 journey (41K → 133K lines) was done in a few conversation sessions.

## Technical Highlights

- **No floating point** — everything uses integer/fixed-point math, including neural network inference
- **No external runtime** — `#![no_std]`, no libc, no POSIX, everything from scratch
- **4 architectures** — same kernel core, per-arch HAL layer with `#[cfg(target_arch)]`
- **UEFI boot** — Limine bootloader, tested on QEMU and real hardware
- **Zero warnings** — entire 133K codebase compiles without a single warning

## Links

- GitHub: https://github.com/MerlionOS/merlion-kernel
- Website: https://merlionos.org
- Release: https://github.com/MerlionOS/merlion-kernel/releases/tag/v100.0.0-final
- License: MIT

## What's next?

The OS works, boots, and has an incredible feature set. But it's still a hobby OS — there are bugs, the VT-x virtualization is simulated, and the compiler can only handle trivial programs. Future directions include real hardware testing on more machines, and possibly a community around it.

## The Singapore Connection

"MerlionOS" is named after the Merlion (鱼尾狮), Singapore's iconic symbol. The tagline "Born for AI. Built by AI" (生于AI，成于AI) reflects that this OS was conceived for AI workloads and constructed through AI collaboration.

---

*Built with Claude Opus by Larry in Singapore*
