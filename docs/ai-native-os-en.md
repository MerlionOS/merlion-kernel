[中文版](ai-native-os.md)

# AI Native OS — MerlionOS Evolution Roadmap

## What is an AI Native OS?

The core abstractions of a traditional OS are **processes, files, and sockets**.
An AI Native OS adds a new core abstraction: **Agents**.

```
Traditional OS:                       AI Native OS:
┌─────────────────────┐           ┌──────────────────────────┐
│  App  App  App      │           │  App  App  Agent  Agent  │
├─────────────────────┤           ├──────────────────────────┤
│  Shell (bash/zsh)   │           │  AI Shell (natural lang) │
├─────────────────────┤           ├──────────────────────────┤
│  Syscalls           │           │  Syscalls + AI Syscalls  │
├─────────────────────┤           ├──────────────────────────┤
│  VFS  Scheduler     │           │  Semantic VFS  AI Sched  │
│  Memory  Drivers    │           │  Memory  Drivers  LLM    │
│                     │           │  Inference Engine        │
└─────────────────────┘           └──────────────────────────┘
```

## Core Philosophy

### 1. LLM as a First-Class System Service

Just as Unix treats "everything is a file" as its core abstraction, an AI Native OS treats **inference capability** as a system service:

- Any process can invoke AI inference through a syscall
- The AI service is always available, just like the VFS
- Inference requests have priority, quotas, and scheduling

### 2. Natural Language as the Primary Interface

```
Traditional:  $ find / -name "*.rs" -mtime -7 | xargs grep "panic" | wc -l
AI Native:    > How many Rust files modified in the last week contain panic?
```

### 3. System Introspection and Self-Healing

- The kernel can explain its own state to users
- Automatic anomaly diagnosis (not just panic, but explaining why it panicked)
- Automatic resource bottleneck identification and suggestions

## Phased Implementation Roadmap

### Phase A: AI Shell (Natural Language Command Interpretation)

**Goal**: Users input natural language, and the OS maps it to existing commands.

```
merlion> show system info
→ Parsed as: neofetch

merlion> what processes are running?
→ Parsed as: ps

merlion> write hello to a temporary file
→ Parsed as: write /tmp/hello hello

merlion> shut down
→ Parsed as: shutdown
```

**Implementation**:
1. Keyword matching engine (in-kernel, no external LLM required)
2. Chinese/English keyword table → command mapping
3. Later integration with a real LLM

**Tech Stack**: Pure Rust, pattern matching, no dependencies

### Phase B: LLM Proxy (Connecting to External LLM via Serial/Network)

**Goal**: The kernel sends prompts to the host machine's LLM via serial port and receives responses.

```
┌─────────────────┐  serial/TCP  ┌──────────────────┐
│   MerlionOS     │ ──────────── │  Host Machine    │
│   (QEMU guest)  │              │  (LLM Proxy)     │
│                 │  prompt →    │  Claude/Ollama   │
│   ai_shell      │  ← response │                  │
└─────────────────┘              └──────────────────┘
```

**Implementation**:
1. Define an AI communication protocol (JSON over serial)
2. Host machine runs a Python/Rust proxy
3. Proxy forwards to Claude API / Ollama / local model
4. Kernel parses the response and executes

**Protocol Design**:
```json
// Request (kernel → host)
{"type": "infer", "id": 1, "prompt": "translate to shell: show memory usage"}

// Response (host → kernel)
{"type": "result", "id": 1, "text": "free", "confidence": 0.95}
```

### Phase C: Semantic File System

**Goal**: Files have not only paths but also semantic tags.

```
merlion> find all files related to networking
→ Searches for files with tags containing "network"

merlion> what does this file do?
→ AI reads the file content and generates a summary
```

**Implementation**:
1. Add a `tags: Vec<String>` field to VFS nodes
2. `tag` / `untag` / `search` commands
3. AI auto-tagging (via LLM proxy)
4. Semantic search (keywords → related files)

### Phase D: AI System Monitoring

**Goal**: AI analyzes system state in real time and detects anomalies.

```
[ai-monitor] Task 'counter' has abnormally high CPU usage (98%), possible infinite loop
[ai-monitor] Heap memory usage reached 90%, recommend checking for memory leaks
[ai-monitor] Abnormal syscall pattern detected: process 5 is calling yield at high frequency
```

**Implementation**:
1. Collect metrics: CPU usage, memory, syscall frequency, page fault rate
2. Rule engine: threshold-based alerts
3. Anomaly detection: statistics-based deviation detection
4. AI explanation: generate human-readable diagnostics via LLM proxy

### Phase E: AI Syscall Interface

**Goal**: User programs can invoke AI through syscalls.

```rust
// In user program:
let answer = syscall::ai_infer("What does this log mean?", log_data);
let embedding = syscall::ai_embed(text);
let similar = syscall::ai_search("network error", top_k=5);
```

**New Syscalls**:
```
7  ai_infer(prompt, context, max_tokens) → response
8  ai_embed(text, len) → embedding_vector
9  ai_search(query, top_k) → results
10 ai_tag(file_path) → tags
```

### Phase F: Self-Healing Kernel

**Goal**: On panic, instead of just printing an error, attempt recovery.

```
Traditional panic:
  KERNEL PANIC: page fault at 0xdeadbeef
  <system halts>

AI Native panic:
  KERNEL PANIC: page fault at 0xdeadbeef
  [ai-diagnosis] Process "counter" accessed an unmapped memory address
  [ai-diagnosis] Probable cause: stack overflow (current stack usage 15.8K / 16K)
  [ai-recovery] Terminated process "counter", system continues running
  [ai-suggestion] Recommend increasing task stack size or checking recursion depth
```

### Phase G: AI Agent Framework

**Goal**: OS-level multi-agent coordination.

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ File Agent   │  │ Net Agent    │  │ Monitor Agent│
│ Manage files │  │ Manage net   │  │ Monitor sys  │
│ Auto-organize│  │ Auto-config  │  │ Auto-repair  │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
┌──────┴─────────────────┴─────────────────┴───────┐
│              Agent Runtime (kernel service)       │
│  Schedule Agents / Manage state / Access control  │
│  / Resource quotas                                │
└──────────────────────────────────────────────────┘
```

## Technical Challenges

### 1. Where Does Inference Run?

| Approach | Pros | Cons |
|----------|------|------|
| **Serial proxy → host LLM** | Simple, can use any model | Depends on host, high latency |
| **Network proxy → cloud API** | Most powerful models | Requires network, has cost |
| **In-kernel tiny model** | Independent, low latency | Limited capability, high memory usage |
| **Dedicated inference virtio device** | Simulates AI accelerator | Requires custom QEMU device |

**Recommended path**: Start with serial proxy (Phase B), then evolve to a virtio AI device.

### 2. Memory Constraints

The current heap is only 64K. Running AI models requires:
- Expanding the heap to 16M+
- Or using demand paging for dynamic allocation
- A tiny model (e.g., quantized TinyLlama) needs ~500MB

### 3. No Floating Point

The kernel has SSE/FPU disabled. Inference requires:
- Enabling SSE in user mode
- Or using pure integer quantized inference
- Or relying entirely on external inference

## Recommended Implementation Order

```
Phase A  ──→  Phase B  ──→  Phase C  ──→  Phase D
  │              │              │              │
Keyword        LLM Proxy     Semantic VFS   AI Monitoring
Cmd Mapping    Serial Proto   File Tags      Anomaly Det.
  │              │              │              │
  └──────────────┴──────────────┴──────────────┘
                        │
                  Phase E ──→ Phase F ──→ Phase G
                    │            │            │
                  AI Syscall   Self-Healing  Agent
                               Kernel       Framework
```

**Phase A can start immediately** — no external dependencies needed, pure in-kernel keyword matching.
**Phase B is the key turning point** — connecting to real LLM capability.

## Comparison with Other AI OS Projects

| Project | Direction | How We Differ |
|---------|-----------|---------------|
| AIOS (Rutgers) | LLM as OS scheduler | We build from the ground-level OS, not wrapping on top of Linux |
| Semantic Kernel (MS) | App-layer AI orchestration | We integrate at the kernel layer |
| Fuchsia + AI | Microkernel + AI services | We are a monolithic kernel, simpler and more direct |
| AutoGPT / CrewAI | App-layer Agents | Our Agents are OS-level |

**MerlionOS's unique positioning**: A teaching OS designed to be AI-aware from the very first line of code. Rather than layering AI on top of an existing OS, AI capability is woven into every layer of the kernel.
