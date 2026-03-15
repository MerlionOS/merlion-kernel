# AI Native OS — MerlionOS 演进路线

## 什么是 AI Native OS？

传统 OS 的核心抽象是**进程、文件、Socket**。
AI Native OS 增加一个新的核心抽象：**智能体 (Agent)**。

```
传统 OS:                          AI Native OS:
┌─────────────────────┐           ┌──────────────────────────┐
│  App  App  App      │           │  App  App  Agent  Agent  │
├─────────────────────┤           ├──────────────────────────┤
│  Shell (bash/zsh)   │           │  AI Shell (自然语言)      │
├─────────────────────┤           ├──────────────────────────┤
│  Syscalls           │           │  Syscalls + AI Syscalls  │
├─────────────────────┤           ├──────────────────────────┤
│  VFS  Scheduler     │           │  Semantic VFS  AI Sched  │
│  Memory  Drivers    │           │  Memory  Drivers  LLM    │
│                     │           │  Inference Engine        │
└─────────────────────┘           └──────────────────────────┘
```

## 核心理念

### 1. LLM 是一等公民系统服务

就像 Unix 把"一切皆文件"作为核心抽象，AI Native OS 把**推理能力**作为系统服务：

- 任何进程都可以通过 syscall 调用 AI 推理
- AI 服务像 VFS 一样始终可用
- 推理请求有优先级、配额、调度

### 2. 自然语言是第一接口

```
传统:    $ find / -name "*.rs" -mtime -7 | xargs grep "panic" | wc -l
AI Native: > 最近一周修改过的 Rust 文件中有多少包含 panic？
```

### 3. 系统自省与自愈

- 内核可以解释自己的状态给用户
- 异常自动诊断（不只是 panic，而是解释为什么 panic）
- 资源瓶颈自动识别和建议

## 分阶段实现路线

### Phase A: AI Shell（自然语言命令解释）

**目标**: 用户输入自然语言，OS 映射到现有命令。

```
merlion> 显示系统信息
→ 解析为: neofetch

merlion> 有哪些进程在运行？
→ 解析为: ps

merlion> 把 hello 写入临时文件
→ 解析为: write /tmp/hello hello

merlion> 关机
→ 解析为: shutdown
```

**实现方式**:
1. 关键词匹配引擎（内核内，无需外部 LLM）
2. 中英文关键词表 → 命令映射
3. 后续接入真正的 LLM

**技术栈**: 纯 Rust，模式匹配，无依赖

### Phase B: LLM 代理（通过串口/网络连接外部 LLM）

**目标**: 内核通过串口把提示发给宿主机的 LLM，接收回复。

```
┌─────────────────┐  serial/TCP  ┌──────────────────┐
│   MerlionOS     │ ──────────── │  Host Machine    │
│   (QEMU guest)  │              │  (LLM Proxy)     │
│                 │  prompt →    │  Claude/Ollama   │
│   ai_shell      │  ← response │                  │
└─────────────────┘              └──────────────────┘
```

**实现方式**:
1. 定义 AI 通信协议 (JSON over serial)
2. 宿主机运行一个 Python/Rust 代理
3. 代理转发到 Claude API / Ollama / 本地模型
4. 内核解析回复并执行

**协议设计**:
```json
// 请求 (kernel → host)
{"type": "infer", "id": 1, "prompt": "translate to shell: 显示内存使用情况"}

// 响应 (host → kernel)
{"type": "result", "id": 1, "text": "free", "confidence": 0.95}
```

### Phase C: 语义文件系统

**目标**: 文件不只有路径，还有语义标签。

```
merlion> 找到所有关于网络的文件
→ 搜索 tags 包含 "network" 的文件

merlion> 这个文件是做什么的？
→ AI 读取文件内容，生成摘要
```

**实现方式**:
1. VFS 节点增加 `tags: Vec<String>` 字段
2. `tag` / `untag` / `search` 命令
3. AI 自动打标签（通过 LLM 代理）
4. 语义搜索（关键词 → 相关文件）

### Phase D: AI 系统监控

**目标**: AI 实时分析系统状态，发现异常。

```
[ai-monitor] Task 'counter' 的 CPU 使用异常高 (98%)，可能是死循环
[ai-monitor] 堆内存使用达到 90%，建议检查内存泄漏
[ai-monitor] 检测到异常 syscall 模式：进程 5 在高频调用 yield
```

**实现方式**:
1. 收集指标：CPU 使用率、内存、syscall 频率、page fault 率
2. 规则引擎：阈值报警
3. 异常检测：基于统计的偏差检测
4. AI 解释：通过 LLM 代理生成人类可读的诊断

### Phase E: AI Syscall 接口

**目标**: 用户程序可以通过 syscall 调用 AI。

```rust
// 用户程序中:
let answer = syscall::ai_infer("这段日志是什么意思？", log_data);
let embedding = syscall::ai_embed(text);
let similar = syscall::ai_search("网络错误", top_k=5);
```

**新增 Syscalls**:
```
7  ai_infer(prompt, context, max_tokens) → response
8  ai_embed(text, len) → embedding_vector
9  ai_search(query, top_k) → results
10 ai_tag(file_path) → tags
```

### Phase F: 自愈内核

**目标**: panic 时不只是打印错误，而是尝试恢复。

```
传统 panic:
  KERNEL PANIC: page fault at 0xdeadbeef
  <system halts>

AI Native panic:
  KERNEL PANIC: page fault at 0xdeadbeef
  [ai-diagnosis] 进程 "counter" 访问了未映射的内存地址
  [ai-diagnosis] 可能原因: 栈溢出（当前栈使用 15.8K / 16K）
  [ai-recovery] 终止了进程 "counter"，系统继续运行
  [ai-suggestion] 建议增大任务栈大小或检查递归深度
```

### Phase G: AI Agent 框架

**目标**: OS 级别的多 Agent 协调。

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ File Agent   │  │ Net Agent    │  │ Monitor Agent│
│ 管理文件      │  │ 管理网络      │  │ 监控系统      │
│ 自动整理      │  │ 自动配置      │  │ 自动修复      │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
┌──────┴─────────────────┴─────────────────┴───────┐
│              Agent Runtime (内核服务)              │
│  调度 Agent / 管理状态 / 权限控制 / 资源配额        │
└──────────────────────────────────────────────────┘
```

## 技术挑战

### 1. 推理在哪里跑？

| 方案 | 优点 | 缺点 |
|------|------|------|
| **串口代理 → 宿主 LLM** | 简单，可用任意模型 | 依赖宿主，延迟高 |
| **网络代理 → 云 API** | 最强模型 | 需要网络，有成本 |
| **内核内 tiny 模型** | 独立，低延迟 | 能力有限，内存占用大 |
| **专用推理 virtio 设备** | 模拟 AI 加速器 | 需要自定义 QEMU 设备 |

**推荐路径**: 先用串口代理（Phase B），再演进到 virtio AI 设备。

### 2. 内存限制

当前堆只有 64K。运行 AI 模型需要：
- 扩大堆到 16M+
- 或使用 demand paging 动态分配
- tiny 模型（如 TinyLlama 量化版）需要 ~500MB

### 3. 无浮点

内核禁用了 SSE/FPU。推理需要：
- 在用户态启用 SSE
- 或使用纯整数量化推理
- 或完全依赖外部推理

## 推荐实施顺序

```
Phase A  ──→  Phase B  ──→  Phase C  ──→  Phase D
  │              │              │              │
关键词          LLM代理        语义VFS        AI监控
命令映射        串口协议        文件标签        异常检测
  │              │              │              │
  └──────────────┴──────────────┴──────────────┘
                        │
                  Phase E ──→ Phase F ──→ Phase G
                    │            │            │
                  AI Syscall   自愈内核     Agent框架
```

**Phase A 可以立即开始** — 不需要外部依赖，纯内核内关键词匹配。
**Phase B 是关键转折点** — 接入真正的 LLM 能力。

## 与其他 AI OS 项目的对比

| 项目 | 方向 | 我们的差异 |
|------|------|-----------|
| AIOS (Rutgers) | LLM as OS scheduler | 我们从底层 OS 做起，不是在 Linux 上包装 |
| Semantic Kernel (MS) | App 层 AI 编排 | 我们在内核层集成 |
| Fuchsia + AI | 微内核 + AI 服务 | 我们是宏内核，更简单直接 |
| AutoGPT / CrewAI | 应用层 Agent | 我们的 Agent 是 OS 级别的 |

**MerlionOS 的独特定位**: 从第一行代码就设计为 AI-aware 的教学型 OS。不是在现有 OS 上叠 AI 层，而是把 AI 能力编织进内核的每一层。
