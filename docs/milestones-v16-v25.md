[English Version](milestones-v16-v25-en.md)

# MerlionOS v16-v25 长期里程碑

> 目标：从 hobby OS 演进为一个真正可用的 AI 原生操作系统平台

---

## v16.0.0 — 音频支持 (Audio)

### 目标
播放系统提示音和简单音频。

### 交付标准
- [ ] Intel HD Audio (HDA) 控制器驱动
- [ ] PCM 音频播放（8-bit/16-bit, mono/stereo）
- [ ] PC Speaker 蜂鸣器（PIT channel 2）
- [ ] `beep` 命令（可设频率和时长）
- [ ] `play` 命令播放简单音调序列
- [ ] 系统事件提示音（启动、错误、通知）

### 预计：~800 行

---

## v17.0.0 — 进程间安全隔离 (Process Isolation)

### 目标
用户进程完全隔离，不能访问其他进程的内存。

### 交付标准
- [ ] Copy-on-Write (CoW) fork
- [ ] 每个进程独立的 VFS 文件描述符表
- [ ] 进程资源限制（最大内存、最大文件打开数）
- [ ] `ulimit` 命令
- [ ] ASLR（地址空间随机化）基础版
- [ ] 进程退出时自动回收所有资源

### 预计：~1200 行

---

## v18.0.0 — 包管理器 (Package Manager)

### 目标
从网络下载和安装软件包。

### 交付标准
- [ ] 包格式定义（.mpkg: 元数据 + ELF 二进制 + 依赖列表）
- [ ] 包仓库协议（HTTP GET from merlionos.dev/packages/）
- [ ] `pkg install <name>` — 下载 + 安装
- [ ] `pkg list` — 列出已安装包
- [ ] `pkg remove <name>` — 卸载
- [ ] 依赖解析（简单版）
- [ ] 签名验证（SHA256 哈希校验）

### 预计：~1000 行

---

## v19.0.0 — Shell 2.0 (Advanced Shell)

### 目标
接近 bash 级别的 shell 体验。

### 交付标准
- [ ] 管道多级链：`cat file | grep x | sort | uniq -c`（已有基础）
- [ ] 后台进程：`command &`
- [ ] 作业控制：`jobs`, `fg %1`, `bg %1`, `Ctrl+Z`
- [ ] 条件执行：`cmd1 && cmd2`, `cmd1 || cmd2`
- [ ] Shell 变量和数组
- [ ] 控制流：`if/then/else/fi`, `for/do/done`, `while/do/done`
- [ ] 函数定义：`function name() { ... }`
- [ ] Here-doc：`cat << EOF`
- [ ] Glob 通配符：`ls /tmp/*.txt`

### 预计：~1500 行

---

## v20.0.0 — 容器化 (Containers)

### 目标
轻量级进程隔离，类似 Docker 的概念。

### 交付标准
- [ ] Namespace 隔离（PID, 文件系统, 网络）
- [ ] `container create <name>` — 创建隔离环境
- [ ] `container exec <name> <cmd>` — 在容器内执行
- [ ] `container list` — 列出容器
- [ ] 每个容器有独立的 VFS 根目录
- [ ] 网络 namespace（独立 IP）
- [ ] 资源 cgroup（CPU 时间, 内存限制）

### 预计：~1500 行

---

## v21.0.0 — 分布式计算 (Distributed)

### 目标
多个 MerlionOS 实例通过网络协同工作。

### 交付标准
- [ ] RPC 框架（序列化 + 网络传输）
- [ ] 服务发现（mDNS/简单广播）
- [ ] `remote exec <host> <cmd>` — 远程执行命令
- [ ] 分布式任务队列
- [ ] 节点状态同步
- [ ] `cluster status` — 查看集群状态

### 预计：~1200 行

---

## v22.0.0 — AI 编程助手 (AI Coding)

### 目标
在 OS 内部用 AI 写代码。

### 交付标准
- [ ] `ai code <描述>` — AI 生成 Forth 代码
- [ ] `ai debug <error>` — AI 分析错误并建议修复
- [ ] `ai optimize <command>` — AI 建议更高效的命令
- [ ] AI 辅助 shell 脚本编写
- [ ] 代码自动补全（基于 AI，不只是前缀匹配）
- [ ] AI 代码审查：分析 /tmp 中的脚本

### 预计：~800 行

---

## v23.0.0 — 多媒体 (Multimedia)

### 目标
基本的图像和文档查看能力。

### 交付标准
- [ ] BMP 图像解析和显示
- [ ] 简单的图像查看器（全屏显示 BMP）
- [ ] 文本文件查看器（带语法高亮）
- [ ] 简单绘图程序（鼠标画线、矩形、填充）
- [ ] 截屏功能（保存帧缓冲为 BMP）
- [ ] `screenshot` 命令

### 预计：~1000 行

---

## v24.0.0 — 虚拟化 (Virtualization)

### 目标
在 MerlionOS 内部运行虚拟机。

### 交付标准
- [ ] 检测 VT-x/AMD-V 支持（CPUID）
- [ ] VMCS/VMCB 基础结构
- [ ] VMX root/non-root 模式切换
- [ ] 运行一个极简的虚拟机（只执行 HLT）
- [ ] `vm create` / `vm start` / `vm stop`
- [ ] 基础 VM Exit 处理

### 预计：~1500 行

---

## v25.0.0 — 自托管 (Self-Hosting)

### 目标
在 MerlionOS 内部编译和运行 Rust 程序。

### 交付标准
- [ ] 移植一个极简 Rust 编译器后端（如 cranelift）
- [ ] 或集成一个解释器（如 WASM 运行时）
- [ ] 在 OS 内编译并运行简单 Rust 程序
- [ ] `rustc hello.rs` → `./hello` 在 MerlionOS 内完成
- [ ] 标准库子集（no_std + alloc）
- [ ] 这是 MerlionOS 的终极目标

### 预计：~3000 行

---

## 代码量预测

```
v15    29,500 行
v16    30,300 行 (音频)
v17    31,500 行 (进程隔离)
v18    32,500 行 (包管理)
v19    34,000 行 (Shell 2.0)
v20    35,500 行 (容器)
v21    36,700 行 (分布式)
v22    37,500 行 (AI 编程)
v23    38,500 行 (多媒体)
v24    40,000 行 (虚拟化)
v25    43,000 行 (自托管)
```

## 愿景

v25.0.0 完成后，MerlionOS 将是：

- **43,000+ 行 Rust** — 一个中等规模的严肃 OS 项目
- 能在**真实硬件**上运行
- 有**图形界面**、**窗口管理器**、**鼠标**
- 有完整的**网络栈**（TCP/IP, HTTP, TLS）
- 有**包管理器**和**容器化**
- **AI 深度集成** — 自主监控、编程辅助、自然语言管理
- 支持**分布式计算**
- 能**自托管** — 在自己内部编译程序
- 完全由 AI 编写 — **Born for AI. Built by AI.**

这不再是一个玩具。这是一个操作系统。 🦁
