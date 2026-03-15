[English Version](milestones-v26-v45-en.md)

# MerlionOS v26-v45 远期里程碑

> 目标：从操作系统演进为 AI 原生计算平台

---

## 第一阶段：系统完善 (v26-v30)

### v26.0.0 — Shell 脚本语言 (Scripting Language)
完整的 shell 脚本语言，支持 if/for/while/function，变量作用域，数组，正则表达式匹配。
**~1200 行**

### v27.0.0 — 权限与安全 (Permissions & Security)
文件权限 (rwx)，用户/组，sudo，capability-based 安全模型，seccomp-like 系统调用过滤。
**~1000 行**

### v28.0.0 — 日志与审计 (Logging & Audit)
结构化日志框架，审计追踪（谁在什么时间做了什么），日志轮转，远程日志（syslog over UDP）。
**~800 行**

### v29.0.0 — 性能分析 (Profiling)
CPU 采样分析器，内存分配追踪，系统调用延迟统计，火焰图生成（文本版），`perf` 命令。
**~1000 行**

### v30.0.0 — 稳定性加固 (Stability Hardening)
内核 watchdog，自动 panic 恢复，内存越界检测（红区），栈溢出保护增强，fuzzing 测试框架。
**~900 行**

---

## 第二阶段：网络服务 (v31-v35)

### v31.0.0 — HTTP 服务器 (Web Server)
内置 HTTP 服务器，静态文件服务，路由，JSON API 端点，`serve` 命令启动，可从浏览器访问。
**~800 行**

### v32.0.0 — SSH 服务器 (Remote Access)
SSH 协议（简化版），密钥认证，远程 shell 会话，`sshd` 后台服务，从其他机器远程登录。
**~1500 行**

### v33.0.0 — DNS 服务器 (DNS Server)
本地 DNS 服务器，支持 A/AAAA/CNAME 记录，zone 文件解析，递归查询。
**~600 行**

### v34.0.0 — MQTT/消息队列 (Message Queue)
轻量级消息代理，发布/订阅模式，主题过滤，持久化消息，IoT 设备通信基础。
**~800 行**

### v35.0.0 — WebSocket 支持 (WebSocket)
WebSocket 协议握手，帧编解码，持久连接，实时数据推送，与 HTTP 服务器集成。
**~600 行**

---

## 第三阶段：AI 平台 (v36-v40)

### v36.0.0 — AI 推理引擎 (Inference Engine)
内核内的量化神经网络推理，支持 ONNX 模型格式（极简子集），INT8 量化矩阵乘法，用于本地 AI 推理。
**~2000 行**

### v37.0.0 — AI 训练基础 (Training Foundation)
简单的梯度下降，线性回归，决策树，能在 OS 内训练小模型。不依赖 GPU。
**~1200 行**

### v38.0.0 — AI 知识库 (Knowledge Base)
向量存储（简单的余弦相似度搜索），文档索引，语义搜索 `/knowledge search "query"`。
**~800 行**

### v39.0.0 — AI 工作流引擎 (AI Workflow)
多步骤 AI 任务编排，AI agent 链式调用，条件分支，结果聚合。类似 LangChain 的概念。
**~1000 行**

### v40.0.0 — AI 自进化 (Self-Evolution)
AI 分析自身代码，建议优化，自动生成补丁，通过测试验证后应用。OS 能改进自己。
**~1500 行**

---

## 第四阶段：硬件扩展 (v41-v45)

### v41.0.0 — GPU 计算 (GPU Compute)
基础 GPU 驱动（virtio-gpu 或简单 VGA），GPU 内存管理，计算着色器接口。
**~1500 行**

### v42.0.0 — 蓝牙支持 (Bluetooth)
USB 蓝牙适配器驱动，HCI 层，L2CAP，蓝牙键盘/鼠标配对。
**~1200 行**

### v43.0.0 — 文件系统集群 (Distributed FS)
网络文件系统，多节点数据复制，一致性协议（简化版 Raft），`mount -t nfs remote:/`。
**~1500 行**

### v44.0.0 — 实时系统 (Real-Time)
实时调度策略（EDF, Rate Monotonic），优先级继承，中断延迟优化，RT 任务类。
**~800 行**

### v45.0.0 — 微内核模式 (Microkernel Mode)
可选的微内核运行模式——驱动运行在用户态，通过 IPC 通信，故障隔离，热重启驱动。
**~2000 行**

---

## 代码量预测

```
v25    43,000 行
v30    47,900 行
v35    51,200 行
v40    57,700 行
v45    64,700 行
```

## 完整路线图概览

| 阶段 | 版本 | 主题 | 目标行数 |
|------|------|------|---------|
| 起步 | v1-v10 | 从零到 AI Native OS | 20K |
| 成长 | v11-v15 | 真机、GUI、联网、生产 | 30K |
| 扩展 | v16-v25 | 音频、容器、虚拟化、自托管 | 43K |
| 完善 | v26-v30 | 脚本、安全、性能、稳定 | 48K |
| 服务 | v31-v35 | HTTP/SSH/DNS/MQTT/WebSocket | 51K |
| AI | v36-v40 | 推理、训练、知识库、自进化 | 58K |
| 硬件 | v41-v45 | GPU、蓝牙、分布式FS、微内核 | 65K |

## 终极愿景

v45.0.0 完成后，MerlionOS 将是：

- **65,000 行 Rust** — 一个真正的中大型操作系统
- **可选微内核模式** — 宏内核和微内核两种运行模式
- **AI 自进化** — 能分析和改进自己的代码
- **GPU 计算** — 支持并行计算
- **分布式** — 多节点协同
- **实时能力** — 可用于嵌入式和 IoT
- **完整网络服务** — HTTP、SSH、DNS 服务器
- **本地 AI 推理** — 不依赖云端的 AI 能力

从一行代码到六万五千行，从一个空目录到一个完整的操作系统平台。

**Born for AI. Built by AI.** 🦁
