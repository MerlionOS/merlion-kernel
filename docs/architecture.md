# MerlionOS Architecture

## Kernel Design: Monolithic Kernel

MerlionOS uses a **monolithic kernel** architecture, the same family as Linux and BSD. All kernel subsystems — scheduler, memory manager, VFS, drivers, IPC, networking — are compiled into a single binary and run in Ring 0 (kernel mode).

```
┌─────────────────────────────────────────────┐
│              User Space (Ring 3)             │
│                                             │
│   User programs (embedded x86_64 code)      │
│   Syscalls via int 0x80                     │
├─────────────────────────────────────────────┤
│              Kernel Space (Ring 0)           │
│                                             │
│   Shell ─ VFS ─ Scheduler ─ Memory Manager  │
│   Drivers ─ IPC ─ Network ─ Framebuffer    │
│   GDT/IDT ─ PIC ─ PIT ─ RTC ─ PCI         │
└─────────────────────────────────────────────┘
│              Hardware                        │
│   CPU ─ RAM ─ VGA ─ Serial ─ Keyboard      │
└─────────────────────────────────────────────┘
```

### Why Monolithic?

- **Simplicity**: all code shares the same address space, no IPC overhead
- **Performance**: function calls between subsystems, not message passing
- **Educational**: easier to understand the full picture in one codebase
- **Pragmatic**: the right choice for a hobby OS in early development

### Trade-offs

- A bug in any subsystem (e.g. a driver) can crash the entire kernel
- All code runs with full hardware access (Ring 0)
- Harder to isolate and restart individual components

## OS Architecture Families

### Monolithic Kernel

Everything in kernel space. Subsystems interact via direct function calls.

| Pros | Cons |
|------|------|
| Fast (no context switch for internal calls) | One bug can crash everything |
| Simple to develop and debug | Large trusted computing base |
| Well-understood, battle-tested | Hard to formally verify |

**Examples**: Linux, FreeBSD, OpenBSD, MerlionOS

### Microkernel

Minimal kernel: only IPC, scheduling, and address space management. Everything else (drivers, filesystems, networking) runs as user-space server processes communicating via message passing.

```
┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐
│ VFS  │ │ NIC  │ │ Disk │ │ App  │  ← User-space servers
│Server│ │Driver│ │Driver│ │      │
└──┬───┘ └──┬───┘ └──┬───┘ └──┬───┘
   │  IPC   │  IPC   │  IPC   │
┌──┴────────┴────────┴────────┴───┐
│       Microkernel (Ring 0)      │
│  Message passing + Scheduler    │
│  + Memory management only       │
└─────────────────────────────────┘
```

| Pros | Cons |
|------|------|
| Fault isolation (driver crash ≠ kernel crash) | IPC overhead on every operation |
| Small kernel = easier to verify | More complex system design |
| Can restart failed services | Harder to achieve high performance |

**Examples**: Mach, L4, seL4, MINIX 3, Google Fuchsia (Zircon)

### Hybrid Kernel

Pragmatic middle ground: microkernel core with some performance-critical subsystems (like drivers or the filesystem) running in kernel space.

| Pros | Cons |
|------|------|
| Balance of isolation and performance | "Worst of both worlds" criticism |
| Flexible architecture | Complex design decisions |

**Examples**: Windows NT, macOS (XNU = Mach + BSD), DragonFly BSD

### Exokernel

Minimal abstraction: the kernel only multiplexes hardware resources (CPU time, memory pages, disk blocks). Applications implement their own OS abstractions via "library operating systems" (libOS).

| Pros | Cons |
|------|------|
| Maximum application control | Every app must implement OS services |
| Can specialize for workload | Not practical for general-purpose use |

**Examples**: MIT Exokernel, Nemesis

### Unikernel

Application and kernel compiled together into a single-purpose image. No separation between user and kernel space. Runs one application directly on the hypervisor.

| Pros | Cons |
|------|------|
| Tiny footprint, fast boot | Single application only |
| Minimal attack surface | No shell, no multi-tenancy |

**Examples**: MirageOS (OCaml), Unikraft, IncludeOS

## MerlionOS Module Map

```
Boot & CPU          Memory              I/O & Drivers
─────────────       ─────────────       ─────────────
gdt.rs              memory.rs           serial.rs
interrupts.rs       allocator.rs        vga.rs
timer.rs            framebuf.rs         keyboard.rs
smp.rs                                  pci.rs
rtc.rs                                  acpi.rs
                                        driver.rs

Process             Filesystem          Communication
─────────────       ─────────────       ─────────────
task.rs             vfs.rs              ipc.rs
process.rs          ramdisk.rs          net.rs
syscall.rs
ulib.rs

Shell & System
─────────────
shell.rs
env.rs
log.rs
testutil.rs
```

## Memory Layout

```
Virtual Address Space (kernel, bootloader 0.9):

High addresses
┌─────────────────────────┐
│ Physical memory mapping  │  ← bootloader maps all physical RAM here
│ (phys_mem_offset + phys) │
├─────────────────────────┤
│ Kernel heap              │  0x4444_4444_0000 (64 KiB)
├─────────────────────────┤
│ Kernel code + data       │  loaded by bootloader
├─────────────────────────┤
│ Kernel stacks (TSS)      │  double fault + ring3 transition
└─────────────────────────┘

User process address space (per-process page table):
┌─────────────────────────┐
│ Upper half: kernel       │  cloned from kernel PML4 (entries 256-511)
├─────────────────────────┤
│ User stack               │  0x800000 (8 KiB, grows down)
├─────────────────────────┤
│ User code                │  0x400000 (mapped from embedded program)
├─────────────────────────┤
│ (unmapped)               │  NULL page protection
└─────────────────────────┘
Low addresses
```

## Syscall ABI

Syscalls use `int 0x80` with a raw naked trampoline that preserves user registers:

```
rax = syscall number
rdi = argument 1
rsi = argument 2
rdx = argument 3

Syscall table:
  0  write(buf, len)        Write to serial + VGA
  1  exit(code)             Terminate process
  2  yield()                Yield to scheduler
  3  getpid()               Get current PID
  4  sleep(ticks)            Sleep for N timer ticks
  5  send(channel, byte)    Send to IPC channel
  6  recv(channel)          Receive from IPC channel
```

## Context Switching

Preemptive round-robin scheduling with cooperative yield support:

1. PIT timer fires at 100 Hz → `timer_handler` → `task::timer_tick()`
2. If other tasks are Ready, calls `yield_now()`
3. `yield_now()` saves callee-saved registers (rbx, rbp, r12-r15) + RSP
4. Swaps RSP to the next task's saved stack pointer
5. Restores registers and `ret` continues the new task

```
context_switch (naked function):
  push rbx, rbp, r12-r15    ; save current task
  mov [rdi], rsp             ; store RSP in old task slot
  mov rsp, rsi               ; load new task's RSP
  pop r15-r12, rbp, rbx      ; restore new task
  ret                        ; resume new task
```

## Future Directions

MerlionOS will continue as a monolithic kernel. Potential future work:

- **Loadable kernel modules**: dynamic driver loading without recompilation
- **Copy-on-write fork**: efficient process creation
- **Demand paging**: lazy memory allocation with page fault handler
- **Virtio drivers**: real hardware interaction (block, network)
- **ELF loader**: load compiled user-space binaries from disk
- **TCP/IP stack**: real networking beyond loopback
- **SMP**: boot application processors, per-CPU run queues

A microkernel variant could be explored as a separate project (MerlionOS-micro) to compare the architectures side by side.
