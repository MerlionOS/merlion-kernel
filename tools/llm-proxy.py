#!/usr/bin/env python3
"""
MerlionOS LLM Proxy — connects the kernel's AI Shell to a real LLM.

The kernel sends JSON requests via QEMU's COM2 serial port.
This proxy reads from a PTY (pseudo-terminal), forwards to an LLM,
and sends the response back.

Usage:
  # 1. Start QEMU with COM2 connected to a PTY:
  qemu-system-x86_64 ... -serial stdio -serial pty

  # 2. QEMU will print something like: "char device redirected to /dev/ttys005"
  #    Use that path:
  python3 tools/llm-proxy.py /dev/ttys005

  # Or with Ollama:
  python3 tools/llm-proxy.py /dev/ttys005 --ollama

  # Or with Claude API:
  export ANTHROPIC_API_KEY=sk-ant-...
  python3 tools/llm-proxy.py /dev/ttys005 --claude

Protocol:
  Request (kernel → proxy):  {"q":"<prompt>"}\\n
  Response (proxy → kernel): {"a":"<answer>"}\\n
"""

import sys
import os
import json
import time
import argparse
import serial


def respond_builtin(prompt: str) -> str:
    """Built-in responses when no LLM is configured."""
    p = prompt.lower().strip()
    if p == "ping":
        return "pong"
    if "hello" in p or "你好" in p:
        return "Hello from the LLM proxy! I'm connected to MerlionOS."
    if "who" in p or "你是" in p:
        return "I'm the MerlionOS AI proxy, bridging the kernel to an LLM."
    if "time" in p or "时间" in p:
        return time.strftime("%Y-%m-%d %H:%M:%S")
    return f"Echo: {prompt} (connect --claude or --ollama for real AI)"


def respond_ollama(prompt: str, model: str = "llama3.2") -> str:
    """Forward to local Ollama instance."""
    try:
        import requests
        resp = requests.post("http://localhost:11434/api/generate", json={
            "model": model,
            "prompt": f"You are MerlionOS AI assistant. Be concise (1-2 sentences). {prompt}",
            "stream": False,
        }, timeout=30)
        data = resp.json()
        return data.get("response", "").strip()[:200]  # truncate for serial
    except Exception as e:
        return f"Ollama error: {e}"


def respond_claude(prompt: str) -> str:
    """Forward to Claude API (requires ANTHROPIC_API_KEY)."""
    try:
        import anthropic
        client = anthropic.Anthropic()
        msg = client.messages.create(
            model="claude-sonnet-4-20250514",
            max_tokens=100,
            system="You are MerlionOS AI assistant embedded in a hobby OS kernel. Be concise (1-2 sentences). Respond in the same language as the user.",
            messages=[{"role": "user", "content": prompt}],
        )
        return msg.content[0].text.strip()[:200]
    except Exception as e:
        return f"Claude API error: {e}"


def respond_claude_code(prompt: str) -> str:
    """Forward to Claude via the `claude` CLI (uses Max/Pro subscription).
    No API key needed — uses your existing Claude Code authentication."""
    import subprocess
    try:
        system_prompt = (
            "You are MerlionOS AI assistant embedded in a hobby OS kernel. "
            "Be concise (1-2 sentences max). "
            "Respond in the same language as the user. "
            "Do not use markdown formatting."
        )
        result = subprocess.run(
            ["claude", "-p", f"{system_prompt}\n\nUser: {prompt}"],
            capture_output=True, text=True, timeout=30,
        )
        answer = result.stdout.strip()
        if not answer:
            answer = result.stderr.strip() or "No response from Claude Code"
        # Truncate for serial transport and strip any markdown
        answer = answer.replace("\n", " ").strip()[:200]
        return answer
    except FileNotFoundError:
        return "Error: 'claude' CLI not found. Install Claude Code first."
    except subprocess.TimeoutExpired:
        return "Claude Code timeout (30s)"
    except Exception as e:
        return f"Claude Code error: {e}"


def main():
    parser = argparse.ArgumentParser(description="MerlionOS LLM Proxy")
    parser.add_argument("port", help="Serial port path (e.g., /dev/ttys005)")
    parser.add_argument("--baud", type=int, default=38400, help="Baud rate")
    parser.add_argument("--claude", action="store_true", help="Use Claude API (needs ANTHROPIC_API_KEY)")
    parser.add_argument("--claude-code", action="store_true", help="Use Claude Code CLI (Max/Pro subscription)")
    parser.add_argument("--ollama", action="store_true", help="Use local Ollama")
    parser.add_argument("--model", default="llama3.2", help="Ollama model name")
    args = parser.parse_args()

    if args.claude_code:
        backend = "Claude Code CLI (Max subscription)"
        respond = respond_claude_code
    elif args.claude:
        backend = "Claude API (ANTHROPIC_API_KEY)"
        respond = respond_claude
    elif args.ollama:
        backend = f"Ollama ({args.model})"
        respond = lambda p: respond_ollama(p, args.model)
    else:
        backend = "built-in echo"
        respond = respond_builtin

    print(f"[llm-proxy] MerlionOS LLM Proxy")
    print(f"[llm-proxy] Port: {args.port}")
    print(f"[llm-proxy] Backend: {backend}")
    print(f"[llm-proxy] Waiting for kernel requests...")

    try:
        ser = serial.Serial(args.port, args.baud, timeout=0.1)
    except Exception as e:
        print(f"[llm-proxy] Error opening {args.port}: {e}")
        print(f"[llm-proxy] Tip: start QEMU with '-serial stdio -serial pty'")
        print(f"[llm-proxy] and use the PTY path QEMU prints.")
        sys.exit(1)

    buf = b""
    while True:
        try:
            data = ser.read(1024)
            if not data:
                continue

            buf += data

            # Look for complete JSON lines
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                line = line.strip()
                if not line:
                    continue

                try:
                    req = json.loads(line)
                    prompt = req.get("q", "")
                    print(f"[llm-proxy] ← {prompt}")

                    answer = respond(prompt)
                    print(f"[llm-proxy] → {answer}")

                    resp = json.dumps({"a": answer}) + "\n"
                    ser.write(resp.encode())
                    ser.flush()

                except json.JSONDecodeError:
                    print(f"[llm-proxy] bad JSON: {line}")

        except KeyboardInterrupt:
            print("\n[llm-proxy] Shutting down.")
            break
        except Exception as e:
            print(f"[llm-proxy] Error: {e}")
            time.sleep(1)

    ser.close()


if __name__ == "__main__":
    main()
