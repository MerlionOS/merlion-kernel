# MerlionOS 下三个里程碑规划

> 当前：v7.0.0 — 87 模块，16,800+ 行 Rust，QEMU 运行

---

## v8.0.0 — 真机启动 (Real Hardware Boot)

### 目标
在 HP 笔记本上从 USB 启动 MerlionOS，进入命令行。

### 交付标准
- [ ] USB 盘启动 → MerlionOS 登录界面
- [ ] 帧缓冲像素渲染（非 VGA 文本模式）
- [ ] 键盘输入可用（PS/2 模拟或 USB）
- [ ] `neofetch` 显示真实 CPU/内存信息
- [ ] QEMU BIOS 模式仍然兼容

### 核心工作

| 任务 | 说明 | 代码量 |
|------|------|--------|
| Limine 引导器集成 | 替换 bootloader 0.9 为 Limine（UEFI+BIOS） | ~300 行 |
| println! 自动适配 | VGA 文本 ↔ 帧缓冲自动切换 | ~50 行（已完成） |
| ISO/USB 构建脚本 | `make iso` → 可启动镜像 | ~100 行 |
| QEMU UEFI 测试 | OVMF 固件 + Limine 验证 | 测试 |
| 真机测试 | HP 笔记本 USB 启动 | 验证 |

### 预计新增代码：~500 行
### 测试环境
- QEMU + OVMF（UEFI 模拟）
- HP 笔记本（关闭 Secure Boot）

---

## v9.0.0 — 端到端联网 (Network End-to-End)

### 目标
在 MerlionOS 中执行 `wget` 从互联网下载网页。

### 交付标准
- [ ] DHCP 自动获取 IP（`ifup` → 分配到 IP）
- [ ] DNS 解析域名（`dns example.com` → IP）
- [ ] TCP 三次握手 + 数据传输
- [ ] `wget http://example.com` 显示 HTML 内容
- [ ] `ping 10.0.2.2` 收到真实回复
- [ ] 在 QEMU user-net 环境下全部工作

### 核心工作

| 任务 | 说明 | 代码量 |
|------|------|--------|
| TCP 完整实现 | 三次握手、数据传输、ACK、FIN、重传 | ~600 行 |
| e1000e RX 中断 | 接收帧 → 解析 → 分发到协议栈 | ~200 行 |
| DHCP 客户端完善 | Discover → Offer → Request → Ack 全流程 | ~200 行 |
| DNS 查询集成 | 通过 UDP 发送 DNS 查询到网关 | ~150 行 |
| wget 实现 | HTTP GET → TCP → 接收响应 → 显示 | ~200 行 |
| netstat 增强 | 显示真实 TCP 连接状态 | ~100 行 |

### 预计新增代码：~1500 行
### 测试环境
- QEMU `-netdev user` (NAT 模式，自带 DHCP/DNS)
- 宿主机运行简单 HTTP server 验证

### 验证方式
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

## v10.0.0 — AI 深度集成 (AI-Powered OS)

### 目标
让 AI 成为操作系统的核心交互方式，而不只是一个附加功能。

### 交付标准
- [ ] `ai` 命令接入真实 Claude（通过网络，不再需要串口代理）
- [ ] 自然语言操作：`ai 找到最大的文件` → 执行搜索
- [ ] AI 自动补全命令（输入前几个字母 → AI 建议完整命令）
- [ ] AI 系统诊断：panic 时自动分析日志并建议修复
- [ ] `ai explain <error>` 解释任何错误信息
- [ ] Agent 自动调度（health agent 定期运行，无需手动）

### 核心工作

| 任务 | 说明 | 代码量 |
|------|------|--------|
| HTTP → Claude API | 通过 TCP/HTTP 直接调用 Claude API | ~300 行 |
| AI 命令执行 | NL → 命令映射 + 自动执行 + 结果反馈 | ~400 行 |
| Tab 补全 + AI 建议 | Tab 键触发 AI 补全建议 | ~200 行 |
| Agent 自动调度 | 后台定时运行 agent（timer tick 驱动） | ~200 行 |
| AI 错误解释 | panic/错误 → 发给 AI → 显示诊断 | ~200 行 |
| AI 配置管理 | API key 存储在 /etc/ai.conf | ~100 行 |

### 预计新增代码：~1400 行
### 前提条件
- v9.0.0 网络栈完成（需要 TCP/HTTP）
- Claude API key（或 Ollama 本地模型）

### 验证方式
```
merlion> ai 查看内存使用情况
[ai] 执行: free
              total       used       free
Phys:      130048 K      512 K   129536 K
Heap:        65536      5280      60256

merlion> ai 创建一个计算斐波那契的脚本
[ai] 已创建 /tmp/fib.sh:
  forth
  : fib dup 2 < if else dup 1 - fib swap 2 - fib + then ;
  10 fib .
  exit

merlion> ai 为什么系统变慢了？
[ai] 分析系统状态...
  - 堆使用 87%（接近满）
  - 5 个任务在运行
  - 建议：运行 'kill 3' 终止不必要的任务，或增大堆大小
```

---

## 时间线总结

```
v8.0.0  真机启动     ~500 行新代码    → 总计 ~17,300 行
v9.0.0  端到端联网   ~1,500 行新代码  → 总计 ~18,800 行
v10.0.0 AI 深度集成  ~1,400 行新代码  → 总计 ~20,200 行
```

完成 v10.0.0 后，MerlionOS 将是一个：
- **能在真机上运行**的操作系统
- **能联网**的操作系统
- **AI 原生**的操作系统
- **20,000+ 行 Rust** 的正经项目

**Born for AI. Built by AI.**
