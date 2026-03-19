# Changelog

## v101.0.0 — Userspace Milestone

**Born for AI. Built by AI.** — 322 source modules, 144K lines of Rust, 71 syscalls, 27 user programs.

### Userspace (U1-U6)

- **U1: Ring 3 Execution** — ELF loader, per-process page tables, iretq to Ring 3
- **U2: Syscall Returns** — all syscall handlers return values via rax
- **U3: exec** — load and run new programs via SYS_EXEC
- **U4: Sockets** — connect/sendto/recvfrom using real TCP stack
- **U5: Minimal libc** — 22 C standard library functions as x86_64 machine code (write, exit, strlen, memcpy, memset, strcmp, malloc, free, open, read, close, getpid, brk, sleep, socket, connect, sendto, recvfrom, gettime, itoa, printf, puts, print_int)
- **U6: Dynamic Linking** — dlopen/dlsym/dlclose, libhello.so + libmath.so

### Process Management

- **Per-process page tables** — CR3 switching for true isolation
- **Copy-on-Write fork** — shared frame tracking, CoW page fault handler
- **Signals** — SYS_SIGACTION/SIGRETURN/KILL, user-mode SIGSEGV
- **Pipes** — SYS_PIPE + SYS_DUP2 via pipefs ring buffers
- **Threads** — SYS_CLONE with shared address space
- **Shared memory** — SYS_SHMGET/SHMAT/SHMDT
- **Per-process fd table** — 32 entries, fork duplicates fds
- **Preemptive user tasks** — spawn-user runs programs as background tasks

### Memory

- **SYS_BRK** — proper heap management with on-demand page mapping
- **SYS_MMAP** — anonymous page mapping
- **SYS_MUNMAP / SYS_MPROTECT** — page release and protection

### I/O & Hardware

- **SYS_FWRITE** — fd-based file writes from userspace
- **SYS_FBWRITE** — framebuffer pixel drawing from Ring 3
- **SYS_BEEP / SYS_PLAY_TONE** — audio from userspace
- **SYS_DISK_READ / SYS_DISK_WRITE** — virtio-blk sector I/O
- **SYS_CPUINFO / SYS_USB_LIST** — hardware query syscalls
- **SYS_TTY_READ** — keyboard input via TTY
- **SYS_WGET** — HTTP fetch from userspace
- **SYS_PRINTF** — kernel-side format string (%d/%x/%s)

### GUI

- **SYS_WIN_CREATE / WIN_PIXEL / WIN_TEXT / WIN_CLOSE** — window management
- **desktop** program — draws title bar, status bar, window on VGA text buffer
- **paint** program — colored rectangles on framebuffer

### Programs (27 built-in)

hello, cat-test, qfc-test, counter, getpid, syscall-test, open-test, exec-test, malloc-test, printf-test, string-test, libc-demo, dynlink-test, cat, echo, wc, ls, init, ush, fwrite-test, paint, wget-user, pkg-install, test-suite, beep, desktop, game

### Self-Hosting

- `compile` shell command — Rust source to ELF via self_host.rs
- ELF from VFS — load programs from /bin filesystem

### Boot & Virtualization

- **Limine UEFI boot fixed** — VGA text buffer guard for UEFI compatibility
- **VMware support** — `make vmdk` + `make vmware-config` (.vmx)
- **BIOS + UEFI** — both boot paths work
- **4-arch CI** — GitHub Actions builds x86_64, aarch64, riscv64, loongarch64

### Documentation

- `docs/syscall-reference.md` — all 71 syscalls documented
- `docs/libmerlion-api.md` — userspace library API reference
- `docs/roadmap-userspace.md` — U1-U6 roadmap
- `CONTRIBUTING.md` — updated contribution guide
