# Roadmap: Rust std Support for MerlionOS

> 让标准 Rust 程序（包括 MerlionClaw）直接在 MerlionOS 上运行

## 架构

```
MerlionClaw (标准 Rust + tokio + axum)
    │
    ▼
x86_64-unknown-merlionos target (自定义 target spec)
    │
    ▼
libstd for merlionos (std shim layer)
    ├── std::net    → SYS_SOCKET/CONNECT/BIND/LISTEN/ACCEPT/SEND/RECV
    ├── std::fs     → SYS_OPEN/READ/WRITE/CLOSE/STAT/MKDIR
    ├── std::thread → SYS_CLONE + stack allocation
    ├── std::sync   → SYS_MUTEX/CONDVAR/FUTEX
    ├── std::io     → SYS_READ/WRITE/PIPE
    ├── std::time   → SYS_CLOCK_MONOTONIC/GETTIMEOFDAY
    ├── std::env    → SYS_GETENV
    └── std::process→ SYS_FORK/EXEC/WAITPID/EXIT
    │
    ▼
MerlionOS Kernel (115 syscalls)
```

## Phase S1: Target Spec + 最小 std (~800行)

1. 创建 `x86_64-unknown-merlionos.json` target spec
2. 实现 `sys/merlionos/` 平台层:
   - syscall 原始接口
   - 进程启动 (_start → main)
   - 基本 I/O (stdin/stdout/stderr)
   - 堆分配 (brk)
   - 退出 (exit)

## Phase S2: 文件系统 + 网络 (~800行)

- std::fs: File, OpenOptions, metadata, read_dir
- std::net: TcpListener, TcpStream, UdpSocket
- std::path: Path, PathBuf

## Phase S3: 线程 + 同步 (~600行)

- std::thread: spawn, join, sleep
- std::sync: Mutex, RwLock, Condvar, Once
- std::sync::mpsc: channel

## Phase S4: 时间 + 环境 + 进程 (~400行)

- std::time: Instant, SystemTime, Duration
- std::env: args, vars, current_dir
- std::process: Command, Child, exit

## Phase S5: 编译 MerlionClaw

```sh
cargo build --target x86_64-unknown-merlionos
# 生成 ELF → 放到 MerlionOS VFS → run-user merlionclaw
```
