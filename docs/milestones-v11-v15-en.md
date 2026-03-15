[中文版](milestones-v11-v15.md)

# MerlionOS v11-v15 Milestone Plan

> Current: v10.0.0 — 99 modules, 20,000+ lines of Rust

---

## v11.0.0 — Real Hardware Networking

### Goal
Connect to the internet from an HP laptop via USB Ethernet adapter.

### Deliverables
- [ ] USB Ethernet adapter driver (CDC-ECM/ASIX)
- [ ] DHCP to obtain real IP address
- [ ] DNS resolution of real domain names
- [ ] `ping 8.8.8.8` succeeds
- [ ] `wget` fetches pages from real websites

### Estimated new code: ~1,500 lines

---

## v12.0.0 — GUI Foundation

### Goal
Simple graphical interface on the pixel framebuffer with windows and mouse.

### Deliverables
- [ ] Window manager (draggable rectangular windows)
- [ ] Mouse cursor (PS/2 or USB HID mouse)
- [ ] Graphical terminal window (shell runs inside a window)
- [ ] Graphical system monitor (visual `top`)
- [ ] Wallpaper (solid color or simple pattern)

### Estimated new code: ~2,000 lines

---

## v13.0.0 — Persistence & Userland

### Goal
Standalone user-space programs, persistent configuration, multi-user support.

### Deliverables
- [ ] Boot from NVMe/AHCI disk (not just USB)
- [ ] ext2-like persistent filesystem
- [ ] Separate merlion-user crate (cross-compiled user programs)
- [ ] User programs loaded from disk (ELF loader + filesystem)
- [ ] `/etc/passwd` multi-user login
- [ ] Password hash verification

### Estimated new code: ~2,500 lines

---

## v14.0.0 — Autonomous AI

### Goal
AI operates autonomously — monitoring, repairing, and optimizing the system without human triggers.

### Deliverables
- [ ] AI calls Claude API over HTTP (HTTPS or HTTP proxy)
- [ ] Agent scheduler runs health/monitor periodically
- [ ] AI auto-detects and kills runaway processes
- [ ] AI adjusts scheduling priority based on load
- [ ] `ai journal` — AI writes periodic system log summaries
- [ ] Natural language system admin: `ai optimize memory` → auto-execute

### Estimated new code: ~1,500 lines

---

## v15.0.0 — Production Ready

### Goal
MerlionOS runs as a lightweight server OS for extended periods.

### Deliverables
- [ ] Stable 24-hour uptime without panic
- [ ] SSH server (or simplified remote shell)
- [ ] Persistent logging to disk
- [ ] Watchdog auto-restart
- [ ] Memory leak detection
- [ ] Complete man pages for all commands
- [ ] One-click disk install (`install` command)
- [ ] Official website at merlionos.dev

### Estimated new code: ~2,000 lines

---

## Code Size Projection

```
v10.0.0   20,000 lines (current)
v11.0.0   21,500 lines
v12.0.0   23,500 lines
v13.0.0   26,000 lines
v14.0.0   27,500 lines
v15.0.0   29,500 lines → approaching 30,000 lines
```

## Ultimate Vision

At v15.0.0, MerlionOS will be:
- An OS that **runs on real hardware for extended periods**
- An OS with a **graphical user interface**
- An OS **autonomously managed by AI**
- A serious open-source project with **~30,000 lines of Rust**
- A technical marvel **written entirely by AI**

**Born for AI. Built by AI.** 🦁
