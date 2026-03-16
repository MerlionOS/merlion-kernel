[中文版](milestones-v78-v100.md)

# MerlionOS v78-v100 Long-Term Roadmap

> From an 82K-line OS platform to a daily-usable system

---

## Phase 1: Real Hardware (v78-v82)

### v78.0.0 — Hardware Compatibility
Fix real-hardware boot issues: USB keyboard hot-plug, ACPI power management,
multiple NIC support (Realtek RTL8139/RTL8169), SATA controller auto-detection.
**Goal: Stable boot on 3+ different PCs**

### v79.0.0 — Display System
Framebuffer graphics rendering engine, font rendering (simplified TrueType),
framebuffer-based terminal emulator, adaptive resolution, multi-monitor detection.
**Goal: Full terminal + GUI on framebuffer**

### v80.0.0 — Input Devices
USB mouse support, touchpad driver (PS/2 Synaptics), multi-key rollover,
keyboard layout switching (US/UK/DE/CN), input method framework.
**Goal: Mouse + keyboard fully functional**

### v81.0.0 — Storage Enhancement
USB mass storage driver, NTFS read-only support, USB drive auto-mount,
disk health check (S.M.A.R.T.), software RAID 0/1.
**Goal: Auto-mount USB storage devices**

### v82.0.0 — Power Management
ACPI S3 sleep/wake, real CPU frequency scaling (P-states),
LCD backlight control, battery level readout (ACPI SBS), low-battery warning.
**Goal: Laptop lid-close sleep, lid-open wake**

---

## Phase 2: User Experience (v83-v87)

### v83.0.0 — Window System
Wayland-inspired compositor, window drag/resize/minimize,
taskbar + system tray, Alt+Tab switching, virtual desktops.
**Goal: Usable graphical desktop environment**

### v84.0.0 — Terminal Emulator
GPU-accelerated terminal (framebuffer), 256-color support, Unicode rendering,
scrollback buffer, selection/copy, tabbed sessions.
**Goal: Modern terminal replacing VGA text mode**

### v85.0.0 — File Manager
Graphical file browser, directory tree navigation, file preview,
context menu, drag-and-drop, file search.
**Goal: Visual file management**

### v86.0.0 — Network Manager
WiFi network list GUI, auto-connect to known networks,
WPA2 password dialog, network status icon, proxy settings.
**Goal: Graphical WiFi connection**

### v87.0.0 — Settings App
Unified settings UI: display, network, sound, power,
user accounts, timezone/language, startup management.
**Goal: One-stop system configuration**

---

## Phase 3: Application Ecosystem (v88-v92)

### v88.0.0 — Web Browser
Simplified HTML renderer (HTML subset + basic CSS),
HTTP/HTTPS client, image display (BMP/PNG),
bookmarks, browsing history.
**Goal: Browse simple web pages**

### v89.0.0 — Email Client
SMTP send, IMAP/POP3 receive, plain-text email,
contact list, attachment support (small files).
**Goal: Send and receive plain-text email**

### v90.0.0 — Music Player
WAV/PCM playback (real HDA output), playlists,
volume control, album art display, equalizer.
**Goal: Play audio through sound card**

### v91.0.0 — Dev Environment
Enhanced editor (vim+), syntax highlighting for more languages,
built-in assembler (x86_64), debugger enhancements (breakpoints + stepping),
simple CI script execution.
**Goal: Write MerlionOS code on MerlionOS**

### v92.0.0 — Package Network
Download packages from HTTP sources, package signature verification,
automatic dependency resolution + install, update checking,
self-hosted package repository server.
**Goal: pkg install hello from the network**

---

## Phase 4: System Maturity (v93-v97)

### v93.0.0 — Multi-User Security
PAM authentication framework, user directory isolation (/home/user),
per-user file encryption, enhanced audit logging,
SSH key authentication (real RSA/Ed25519).
**Goal: Multi-user security isolation**

### v94.0.0 — Container Runtime
OCI-compatible container runtime, container images (simplified),
cgroup resource isolation, network namespaces,
container orchestration (simplified docker-compose).
**Goal: Run simple containerized applications**

### v95.0.0 — Virtualization
KVM-like hypervisor (VT-x),
guest memory management (EPT),
virtual devices (serial, disk, NIC),
run MerlionOS inside MerlionOS.
**Goal: Nested virtualization**

### v96.0.0 — Network File System (NFS)
NFSv3 client, transparent remote file access,
auto-mount, file caching, offline mode.
**Goal: mount -t nfs server:/share /mnt**

### v97.0.0 — Performance Optimization
Kernel hot-path optimization, zero-copy networking,
transparent huge pages (THP), IO scheduler (CFQ/BFQ),
benchmark suite + performance regression detection.
**Goal: Network throughput and latency at usable levels**

---

## Phase 5: AI Evolution (v98-v100)

### v98.0.0 — Local LLM Inference
GGUF model loader, INT4/INT8 quantized inference,
KV cache, simplified attention mechanism,
run sub-1B parameter models on CPU.
**Goal: Offline AI chat (small models)**

### v99.0.0 — AI-Driven SysAdmin
AI automatic system diagnostics, intelligent log analysis,
automatic performance tuning, anomaly detection and alerts,
natural language system configuration.
**Goal: Manage the OS with natural language**

### v100.0.0 — Self-Hosting
Compile MerlionOS on MerlionOS,
integrated Rust compiler frontend (simplified),
bootstrapping compilation chain,
complete develop-compile-test-deploy cycle.
**Goal: The OS compiles itself — the ultimate milestone**

---

## Growth Projection

```
v77     82,000 lines  — current
v82     95,000 lines  — real hardware
v87    115,000 lines  — user experience
v92    140,000 lines  — applications
v97    170,000 lines  — system maturity
v100   200,000 lines  — self-hosting
```

## Ultimate Vision

After v100.0.0, MerlionOS will be:

- **200,000 lines of Rust** — a real medium-to-large OS
- **Self-hosting** — an OS that compiles itself
- **Graphical desktop** — window manager, terminal, file manager
- **Web browsing** — simple HTML browser
- **Local AI** — offline LLM inference
- **Containers + virtualization** — isolated apps and VMs
- **Real hardware** — daily use on standard x86_64 laptops

From one line of code to two hundred thousand. From an empty directory to a complete operating system.

**Born for AI. Built by AI.**
