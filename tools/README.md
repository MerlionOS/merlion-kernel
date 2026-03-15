# MerlionOS Tools

## LLM Proxy (`llm-proxy.py`)

Connects the kernel's AI Shell to a real LLM via QEMU's COM2 serial port.

### Prerequisites

```sh
pip install pyserial anthropic  # or: pip install pyserial requests (for Ollama)
```

### Usage

**Terminal 1** — Start QEMU with dual serial:
```sh
make run-ai
```

QEMU will print: `char device redirected to /dev/ttysXXX`

**Terminal 2** — Start the proxy with that path:
```sh
# Recommended: Use your Claude Max/Pro subscription (no API key needed):
python3 tools/llm-proxy.py /dev/ttysXXX --claude-code

# With Claude API (needs separate API key):
export ANTHROPIC_API_KEY=sk-ant-...
python3 tools/llm-proxy.py /dev/ttysXXX --claude

# With local Ollama (free, offline):
python3 tools/llm-proxy.py /dev/ttysXXX --ollama

# Without LLM (echo mode for testing):
python3 tools/llm-proxy.py /dev/ttysXXX
```

**In MerlionOS shell:**
```
merlion> ai what is Singapore famous for?
[ai] Singapore is known for its Merlion statue, Marina Bay Sands, and...

merlion> ai 用中文介绍一下新加坡
[ai] 新加坡是一个位于东南亚的城市国家...
```

### Protocol

```
Kernel → Proxy:  {"q":"<prompt>"}\n
Proxy → Kernel:  {"a":"<answer>"}\n
```

### Backends

| Flag | Backend | Requirements |
|------|---------|-------------|
| `--claude-code` | **Claude Code CLI** | Claude Max/Pro subscription |
| `--claude` | Claude API | `ANTHROPIC_API_KEY` env var |
| `--ollama` | Local Ollama | `ollama serve` running |
| (none) | Built-in echo | None |
