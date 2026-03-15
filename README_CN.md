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

## 当前状态（第 42 阶段）

- **47 个源码模块，约 7500 行 Rust 代码**
- **42 个内核阶段 + 7 个 AI 阶段已完成**
- **75+ 个 Shell 命令**
- AI 原生宏内核：自然语言 Shell、LLM 代理、语义 VFS、AI 监控、自愈内核、Agent 框架

## 技术架构

详见 [架构文档](docs/architecture.md) 和 [AI Native OS 规划](docs/ai-native-os.md)。

## 路线图

### 1. 基础（第 1-10 阶段）✅
启动、GDT/IDT、PIT、键盘、堆/帧分配器、VGA 控制台、Shell、抢占式多任务、用户态 Ring 3、进程页表、系统调用、IPC、VFS、ACPI

### 2. 功能（第 11-20 阶段）✅
RTC 时钟、内核自测、帧缓冲图形、PCI 总线扫描、RAM 磁盘、IPv4/UDP 网络、CPUID/SMP、用户态库、命令历史、方向键、环境变量、别名、neofetch

### 3. 内核演进（第 21-25 阶段）✅
可加载内核模块、按需分页、内核符号表+栈回溯、Slab 分配器

### 4. 硬件支持（第 26-30 阶段）✅
Virtio 设备发现、块设备抽象、FAT16 文件系统、ARP 表、ICMP Ping、TCP 状态机

### 5. 用户空间（第 31-35 阶段）✅
文件描述符表、stdin/stdout/stderr、用户态系统调用库

### 6. SMP 与高级特性（第 36-40 阶段）✅
CPUID/APIC 检测、每 CPU 状态、自旋锁 vs 票据锁、APIC 定时器校准

### 7. AI Native OS（第 A-G 阶段）✅
自然语言 Shell（中英文）、LLM 串口代理、语义文件系统、AI 系统监控、AI 系统调用（推理/分类/解释）、自愈内核、Agent 框架

### 8. Shell 与脚本（第 41-42 阶段）✅
Shell 脚本执行、分号链式命令、wc、AI 增强 panic 诊断

### 9. 加固与完善（第 43-50 阶段）— 计划中

| 阶段 | 内容 |
|------|------|
| 43 | 栈溢出保护（Guard pages）|
| 44 | 堆加固（double-free 检测）|
| 45 | `/proc/self` 及每进程 proc 条目 |
| 46 | VFS 挂载点（mount/umount）|
| 47 | 信号框架（SIGKILL、SIGTERM、SIGSTOP）|
| 48 | 作业控制（fg、bg、jobs、Ctrl+C）|
| 49 | Shell 管道：`cmd1 \| cmd2` |
| 50 | 内核配置系统（/etc/merlion.conf）|

### 10. 真实 I/O（第 51-55 阶段）— 计划中

| 阶段 | 内容 |
|------|------|
| 51 | Virtio-blk 驱动（真实 QEMU 磁盘 I/O）|
| 52 | Virtio-blk 上的持久化文件系统 |
| 53 | Virtio-net 驱动（真实以太网帧）|
| 54 | 真实网卡上的 ARP + ICMP |
| 55 | 最小 TCP 栈（三次握手 + 数据传输）|

### 11. 完整用户态（第 56-60 阶段）— 计划中

| 阶段 | 内容 |
|------|------|
| 56 | ELF 二进制解析器和加载器 |
| 57 | 独立 merlion-user crate（交叉编译）|
| 58 | 用户态 libc：malloc、printf、字符串操作 |
| 59 | Init 进程 + 多用户登录 |
| 60 | 用户态 Shell（msh 作为独立二进制）|

### 12. AI 深度集成（第 61-65 阶段）— 计划中

| 阶段 | 内容 |
|------|------|
| 61 | Virtio AI 设备（自定义 QEMU 推理设备）|
| 62 | AI 辅助任务调度（负载预测）|
| 63 | 自然语言 VFS 查询（"找到大文件"）|
| 64 | AI 驱动的 man 手册（解释任何命令）|
| 65 | 会话式系统管理 Agent |

### 13. 未来展望（第 66-70 阶段）— 计划中

| 阶段 | 内容 |
|------|------|
| 66 | UEFI 启动（替换 BIOS 引导）|
| 67 | x86_64 → aarch64 跨架构移植 |
| 68 | 帧缓冲 GUI：窗口管理器、鼠标支持 |
| 69 | USB HID 驱动（键盘/鼠标）|
| 70 | 自托管：在 MerlionOS 内编译 Rust |
