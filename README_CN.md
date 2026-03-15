# MerlionOS

> **Born for AI. Built by AI.** — 生于AI，成于AI

[English](README.md)

新加坡主题的 AI 原生 hobby 操作系统，使用 Rust 编写，运行在 x86_64 架构上。

## 环境准备

- **Rust nightly**（通过 `rust-toolchain.toml` 自动管理）
- **rust-src**: `rustup component add rust-src --toolchain nightly`
- **llvm-tools**: `rustup component add llvm-tools --toolchain nightly`
- **cargo-bootimage**: `cargo install bootimage`
- **QEMU**: `brew install qemu`（macOS）或 `apt install qemu-system-x86`（Linux）

## 构建与运行

```sh
make build          # 构建可启动镜像
make run            # 在 QEMU 中启动（VGA 窗口 + 串口）
make run-serial     # 无界面模式（仅串口输出）
make run-fullscreen # 全屏模式（按 Ctrl+Option+F 退出全屏）
```

## Shell 命令

### 进程管理
| 命令 | 说明 |
|------|------|
| `ps` | 列出运行中的任务 |
| `spawn` | 启动一个演示任务 |
| `kill <pid>` | 终止指定任务 |
| `bg <程序>` | 后台运行用户程序 |
| `run <程序>` | 前台运行用户程序 |
| `progs` | 列出可用用户程序 |

### 文件操作
| 命令 | 说明 |
|------|------|
| `ls [路径]` | 列出目录内容（默认: /）|
| `cat <路径>` | 读取文件内容 |
| `write <路径> <数据>` | 写入文件 |
| `rm <路径>` | 删除文件 |
| `open <路径>` | 打开文件描述符 |
| `close <fd>` | 关闭文件描述符 |
| `lsof` | 列出打开的文件描述符 |

### 系统信息
| 命令 | 说明 |
|------|------|
| `info` | 系统信息 |
| `neofetch` | 系统概览（带 Logo）|
| `date` | 当前日期时间 |
| `uptime` | 运行时间 |
| `uname` | 内核版本 |
| `whoami` | 当前用户 |
| `hostname` | 主机名 |
| `cpuinfo` | CPU 特性与核心数 |
| `free` | 内存使用摘要 |
| `heap` | 堆分配器统计 |
| `memmap` | 物理内存映射（彩色）|
| `drivers` | 内核驱动列表 |
| `lspci` | PCI 设备列表 |

### 内核模块
| 命令 | 说明 |
|------|------|
| `lsmod` | 列出内核模块 |
| `modprobe <模块>` | 加载模块 |
| `rmmod <模块>` | 卸载模块 |
| `modinfo <模块>` | 模块详细信息 |

### 网络
| 命令 | 说明 |
|------|------|
| `ifconfig` | 网络接口信息 |
| `ping <地址>` | Ping 一个地址 |
| `arp` | ARP 表 |
| `send <消息>` | 发送 UDP 环回包 |
| `recv` | 接收排队的数据包 |

### 存储
| 命令 | 说明 |
|------|------|
| `disk` | RAM 磁盘状态 |
| `format` | 格式化 RAM 磁盘（MRLN 格式）|
| `fatfmt` | 格式化为 MF16 文件系统 |
| `fatls` / `fatr` / `fatw` | MF16 文件操作 |
| `blkdevs` | 块设备列表 |

### 其他
| 命令 | 说明 |
|------|------|
| `echo <消息>` | 打印消息 |
| `env` | 环境变量 |
| `set K=V` | 设置变量 |
| `alias n=c` | 设置别名 |
| `history` | 命令历史 |
| `sleep <秒>` | 休眠 N 秒 |
| `gfx` | 图形演示（新加坡国旗）|
| `test` | 运行内核自测（15项）|
| `slabinfo` | Slab 分配器统计 |
| `lockdemo` | 自旋锁 vs 票据锁对比 |
| `bt` | 内核栈回溯 |
| `dmesg` | 内核日志缓冲 |
| `clear` | 清屏 |
| `shutdown` | 关机（ACPI）|
| `reboot` | 重启 |

## 虚拟文件系统

```
/
├── dev/
│   ├── null       # 丢弃输出
│   └── serial     # COM1 串口
├── proc/
│   ├── uptime     # 运行时间
│   ├── meminfo    # 内存统计
│   └── tasks      # 任务列表
└── tmp/           # 可写用户文件
```

## 系统调用 ABI (int 0x80)

| 编号 | 名称 | 参数 | 说明 |
|------|------|------|------|
| 0 | write | rdi=缓冲区, rsi=长度 | 输出到串口+VGA |
| 1 | exit | rdi=退出码 | 终止进程 |
| 2 | yield | — | 让出 CPU |
| 3 | getpid | — | 获取当前 PID |
| 4 | sleep | rdi=时钟滴答数 | 休眠 |
| 5 | send | rdi=通道, rsi=字节 | 发送到 IPC 通道 |
| 6 | recv | rdi=通道 | 从 IPC 通道接收 |

## 当前状态（第 40 阶段 — 全部完成）

- **39 个源码模块，约 6200 行 Rust 代码**
- **40 个开发阶段全部完成，跨越 5 个里程碑**
- **65+ 个 Shell 命令**
- 宏内核架构，抢占式多任务，用户态，VFS，IPC，网络，
  FAT16，图形，PCI，CPUID，APIC 定时器，可加载模块，
  按需分页，Slab 分配器，文件描述符

## 技术架构

详见 [架构文档](docs/architecture.md) 和 [AI Native OS 规划](docs/ai-native-os.md)。

## 路线图

### 基础（第 1-10 阶段）— ✅ 已完成
启动、GDT/IDT、PIT 定时器、键盘、堆分配器、帧分配器、VGA 控制台、Shell、抢占式多任务、用户态、进程页表、系统调用、IPC、VFS、ACPI

### 功能（第 11-20 阶段）— ✅ 已完成
RTC 时钟、内核自测、帧缓冲图形、PCI 总线扫描、RAM 磁盘、IPv4/UDP 网络、CPUID/SMP、用户态库、命令历史、方向键、环境变量、别名、neofetch

### 内核演进（第 21-25 阶段）— ✅ 已完成
可加载内核模块、按需分页、内核符号表+栈回溯、Slab 分配器

### 硬件支持（第 26-30 阶段）— ✅ 已完成
Virtio 设备发现、块设备抽象、FAT16 文件系统、ARP 表、ICMP Ping、TCP 状态机

### 用户空间（第 31-35 阶段）— ✅ 已完成
用户态系统调用库、文件描述符表、POSIX 风格 fd 命令、stdin/stdout/stderr

### SMP 与高级特性（第 36-40 阶段）— ✅ 已完成
AP 启动类型、每 CPU 状态跟踪、自旋锁+票据锁、APIC 定时器校准

### AI Native OS（规划中）
自然语言 Shell、LLM 代理、语义文件系统、AI 系统监控、AI 系统调用、自愈内核、Agent 框架

详见 [AI Native OS 规划文档](docs/ai-native-os.md)。
