[中文版](milestones-v16-v25.md)

# MerlionOS v16-v25 Long-Term Milestones

> Goal: Evolve from hobby OS to a truly usable AI-native operating system platform

---

## v16.0.0 — Audio Support

### Goal
Play system sounds and simple audio.

### Deliverables
- [ ] Intel HD Audio (HDA) controller driver
- [ ] PCM audio playback (8/16-bit, mono/stereo)
- [ ] PC Speaker beeper (PIT channel 2)
- [ ] `beep` command (configurable frequency and duration)
- [ ] `play` command for simple tone sequences
- [ ] System event sounds (boot, error, notification)

### Estimated: ~800 lines

---

## v17.0.0 — Process Isolation

### Goal
Full memory isolation between user processes.

### Deliverables
- [ ] Copy-on-Write (CoW) fork
- [ ] Per-process file descriptor tables
- [ ] Process resource limits (max memory, max open files)
- [ ] `ulimit` command
- [ ] Basic ASLR (Address Space Layout Randomization)
- [ ] Automatic resource reclamation on process exit

### Estimated: ~1,200 lines

---

## v18.0.0 — Package Manager

### Goal
Download and install software packages from the network.

### Deliverables
- [ ] Package format (.mpkg: metadata + ELF binary + dependency list)
- [ ] Repository protocol (HTTP GET from merlionos.dev/packages/)
- [ ] `pkg install <name>` — download + install
- [ ] `pkg list` — list installed packages
- [ ] `pkg remove <name>` — uninstall
- [ ] Dependency resolution (simple version)
- [ ] Signature verification (SHA256 hash check)

### Estimated: ~1,000 lines

---

## v19.0.0 — Shell 2.0

### Goal
Near-bash-level shell experience.

### Deliverables
- [ ] Multi-stage pipes: `cat file | grep x | sort | uniq -c`
- [ ] Background processes: `command &`
- [ ] Job control: `jobs`, `fg %1`, `bg %1`, `Ctrl+Z`
- [ ] Conditional execution: `cmd1 && cmd2`, `cmd1 || cmd2`
- [ ] Shell variables and arrays
- [ ] Control flow: `if/then/else/fi`, `for/do/done`, `while/do/done`
- [ ] Function definitions: `function name() { ... }`
- [ ] Here-documents: `cat << EOF`
- [ ] Glob wildcards: `ls /tmp/*.txt`

### Estimated: ~1,500 lines

---

## v20.0.0 — Containers

### Goal
Lightweight process isolation, Docker-like concepts.

### Deliverables
- [ ] Namespace isolation (PID, filesystem, network)
- [ ] `container create <name>` — create isolated environment
- [ ] `container exec <name> <cmd>` — execute inside container
- [ ] `container list` — list containers
- [ ] Independent VFS root per container
- [ ] Network namespace (independent IP)
- [ ] Resource cgroups (CPU time, memory limits)

### Estimated: ~1,500 lines

---

## v21.0.0 — Distributed Computing

### Goal
Multiple MerlionOS instances collaborate over the network.

### Deliverables
- [ ] RPC framework (serialization + network transport)
- [ ] Service discovery (mDNS/simple broadcast)
- [ ] `remote exec <host> <cmd>` — remote command execution
- [ ] Distributed task queue
- [ ] Node state synchronization
- [ ] `cluster status` — view cluster state

### Estimated: ~1,200 lines

---

## v22.0.0 — AI Coding Assistant

### Goal
Write code inside the OS using AI.

### Deliverables
- [ ] `ai code <description>` — AI generates Forth code
- [ ] `ai debug <error>` — AI analyzes errors and suggests fixes
- [ ] `ai optimize <command>` — AI suggests more efficient commands
- [ ] AI-assisted shell script writing
- [ ] AI-powered code completion (beyond prefix matching)
- [ ] AI code review: analyze scripts in /tmp

### Estimated: ~800 lines

---

## v23.0.0 — Multimedia

### Goal
Basic image and document viewing capabilities.

### Deliverables
- [ ] BMP image parsing and display
- [ ] Simple image viewer (fullscreen BMP display)
- [ ] Text file viewer with syntax highlighting
- [ ] Simple drawing program (mouse: lines, rectangles, fill)
- [ ] Screenshot capture (save framebuffer as BMP)
- [ ] `screenshot` command

### Estimated: ~1,000 lines

---

## v24.0.0 — Virtualization

### Goal
Run virtual machines inside MerlionOS.

### Deliverables
- [ ] Detect VT-x/AMD-V support (CPUID)
- [ ] Basic VMCS/VMCB structures
- [ ] VMX root/non-root mode transitions
- [ ] Run a minimal VM (execute HLT only)
- [ ] `vm create` / `vm start` / `vm stop`
- [ ] Basic VM Exit handling

### Estimated: ~1,500 lines

---

## v25.0.0 — Self-Hosting

### Goal
Compile and run Rust programs inside MerlionOS.

### Deliverables
- [ ] Port a minimal Rust compiler backend (e.g., cranelift)
- [ ] Or integrate a runtime (e.g., WASM interpreter)
- [ ] Compile and run simple Rust programs inside the OS
- [ ] `rustc hello.rs` → `./hello` entirely within MerlionOS
- [ ] Standard library subset (no_std + alloc)
- [ ] This is MerlionOS's ultimate goal

### Estimated: ~3,000 lines

---

## Code Size Projection

```
v15    29,500 lines
v16    30,300 lines (audio)
v17    31,500 lines (process isolation)
v18    32,500 lines (package manager)
v19    34,000 lines (shell 2.0)
v20    35,500 lines (containers)
v21    36,700 lines (distributed)
v22    37,500 lines (AI coding)
v23    38,500 lines (multimedia)
v24    40,000 lines (virtualization)
v25    43,000 lines (self-hosting)
```

## Vision

At v25.0.0, MerlionOS will be:

- **43,000+ lines of Rust** — a medium-scale serious OS project
- Runs on **real hardware**
- Has a **graphical interface**, **window manager**, **mouse**
- Complete **network stack** (TCP/IP, HTTP, TLS)
- Has a **package manager** and **containers**
- **Deep AI integration** — autonomous monitoring, coding assistant, NL admin
- Supports **distributed computing**
- Can **self-host** — compile programs within itself
- Written entirely by AI — **Born for AI. Built by AI.**

This is no longer a toy. This is an operating system. 🦁
