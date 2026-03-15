[中文版](milestones-v8-v10.md)

# MerlionOS Next Three Milestone Plan

> Current: v7.0.0 — 87 modules, 16,800+ lines of Rust, running on QEMU

---

## v8.0.0 — Real Hardware Boot

### Goal
Boot MerlionOS from USB on an HP laptop and reach the command line.

### Deliverables
- [ ] USB drive boot → MerlionOS login screen
- [ ] Framebuffer pixel rendering (not VGA text mode)
- [ ] Keyboard input functional (PS/2 emulation or USB)
- [ ] `neofetch` displays real CPU/memory information
- [ ] QEMU BIOS mode remains compatible

### Core Work

| Task | Description | Code Size |
|------|-------------|-----------|
| Limine bootloader integration | Replace bootloader 0.9 with Limine (UEFI+BIOS) | ~300 lines |
| println! auto-adaptation | VGA text ↔ framebuffer automatic switching | ~50 lines (done) |
| ISO/USB build script | `make iso` → bootable image | ~100 lines |
| QEMU UEFI testing | OVMF firmware + Limine verification | Testing |
| Real hardware testing | HP laptop USB boot | Validation |

### Estimated New Code: ~500 lines
### Test Environment
- QEMU + OVMF (UEFI emulation)
- HP laptop (Secure Boot disabled)

---

## v9.0.0 — Network End-to-End

### Goal
Execute `wget` inside MerlionOS to download a web page from the internet.

### Deliverables
- [ ] DHCP auto-acquires IP (`ifup` → IP assigned)
- [ ] DNS resolves domain names (`dns example.com` → IP)
- [ ] TCP three-way handshake + data transfer
- [ ] `wget http://example.com` displays HTML content
- [ ] `ping 10.0.2.2` receives real replies
- [ ] Everything works under QEMU user-net

### Core Work

| Task | Description | Code Size |
|------|-------------|-----------|
| Full TCP implementation | Three-way handshake, data transfer, ACK, FIN, retransmission | ~600 lines |
| e1000e RX interrupts | Receive frames → parse → dispatch to protocol stack | ~200 lines |
| DHCP client completion | Discover → Offer → Request → Ack full flow | ~200 lines |
| DNS query integration | Send DNS queries to gateway via UDP | ~150 lines |
| wget implementation | HTTP GET → TCP → receive response → display | ~200 lines |
| netstat enhancement | Display real TCP connection states | ~100 lines |

### Estimated New Code: ~1500 lines
### Test Environment
- QEMU `-netdev user` (NAT mode, built-in DHCP/DNS)
- Host machine running a simple HTTP server for verification

### Validation
```
merlion> ifup
DHCP: obtained 10.0.2.15/24, gateway 10.0.2.2, DNS 10.0.2.3

merlion> dns example.com
example.com → 93.184.216.34

merlion> wget http://10.0.2.2:8080/
HTTP/1.1 200 OK
Hello from MerlionOS network stack!

merlion> ping 10.0.2.2
Reply from 10.0.2.2: seq=0 time=1ms
```

---

## v10.0.0 — AI-Powered OS

### Goal
Make AI a core interaction method of the operating system, not just an add-on feature.

### Deliverables
- [ ] `ai` command connects to real Claude (via network, no serial proxy needed)
- [ ] Natural language operations: `ai find the largest file` → executes search
- [ ] AI command auto-completion (type first few characters → AI suggests full command)
- [ ] AI system diagnostics: automatically analyzes logs on panic and suggests fixes
- [ ] `ai explain <error>` explains any error message
- [ ] Agent auto-scheduling (health agent runs periodically without manual invocation)

### Core Work

| Task | Description | Code Size |
|------|-------------|-----------|
| HTTP → Claude API | Call Claude API directly via TCP/HTTP | ~300 lines |
| AI command execution | NL → command mapping + auto-execution + result feedback | ~400 lines |
| Tab completion + AI suggestions | Tab key triggers AI completion suggestions | ~200 lines |
| Agent auto-scheduling | Background timed agent execution (timer tick driven) | ~200 lines |
| AI error explanation | panic/error → send to AI → display diagnosis | ~200 lines |
| AI configuration management | API key stored in /etc/ai.conf | ~100 lines |

### Estimated New Code: ~1400 lines
### Prerequisites
- v9.0.0 network stack complete (requires TCP/HTTP)
- Claude API key (or Ollama local model)

### Validation
```
merlion> ai check memory usage
[ai] Executing: free
              total       used       free
Phys:      130048 K      512 K   129536 K
Heap:        65536      5280      60256

merlion> ai create a script that computes Fibonacci numbers
[ai] Created /tmp/fib.sh:
  forth
  : fib dup 2 < if else dup 1 - fib swap 2 - fib + then ;
  10 fib .
  exit

merlion> ai why is the system slow?
[ai] Analyzing system state...
  - Heap usage at 87% (near full)
  - 5 tasks running
  - Suggestion: run 'kill 3' to terminate unnecessary tasks, or increase heap size
```

---

## Timeline Summary

```
v8.0.0  Real Hardware Boot    ~500 new lines   → Total ~17,300 lines
v9.0.0  Network End-to-End  ~1,500 new lines   → Total ~18,800 lines
v10.0.0 AI-Powered OS       ~1,400 new lines   → Total ~20,200 lines
```

After completing v10.0.0, MerlionOS will be:
- An operating system that **runs on real hardware**
- An operating system that **connects to the network**
- An **AI-native** operating system
- A serious project with **20,000+ lines of Rust**

**Born for AI. Built by AI.**
