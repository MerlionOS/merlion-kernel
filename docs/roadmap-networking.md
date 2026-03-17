[English Version](roadmap-networking-en.md)

# MerlionOS 网络子系统路线图

> 从基础TCP/IP到企业级网络栈

---

## 现有网络能力 (已完成)

```
┌─────────────────────────────────────────────────┐
│ 应用层                                           │
│ HTTP/1.1 · HTTP/2 · HTTP/3 · FTP · SSH · MQTT  │
│ WebSocket · DNS · DHCP · NTP · mDNS · DNS-SD   │
├─────────────────────────────────────────────────┤
│ 安全层                                           │
│ TLS 1.3 · WireGuard · AES-128 · RSA · X.509    │
│ ChaCha20-Poly1305 · PBKDF2                      │
├─────────────────────────────────────────────────┤
│ 传输层                                           │
│ TCP (Reno/Cubic/BBR) · QUIC · UDP               │
├─────────────────────────────────────────────────┤
│ 网络层                                           │
│ IPv4 · IPv6 · iptables/NAT · VLAN 802.1Q       │
│ ICMP · ICMPv6 · NDP · ARP                       │
├─────────────────────────────────────────────────┤
│ 链路层                                           │
│ e1000e · virtio-net · veth · bridge · WiFi      │
├─────────────────────────────────────────────────┤
│ 工具                                             │
│ ss · nc · ip · traceroute · portscan · ping     │
│ ifconfig · netstat · arp · dns                   │
└─────────────────────────────────────────────────┘
```

---

## 第一阶段：TCP增强 (N1)

### N1.1 — TCP Fast Open (TFO)
- SYN包携带数据，0-RTT建连
- TFO Cookie生成和验证
- 客户端/服务端双端支持
- `sysctl net.tcp.fastopen=3`
- **~150行**

### N1.2 — TCP SACK (Selective ACK)
- 选择性确认，避免重传已收到的包
- SACK选项解析和生成
- SACK-based重传策略
- **~200行**

### N1.3 — TCP Window Scaling
- 窗口缩放选项（RFC 7323）
- 支持>64KB接收窗口
- 自动协商缩放因子
- **~100行**

### N1.4 — TCP Timestamps
- 时间戳选项（RFC 7323）
- RTTM (Round-Trip Time Measurement)
- PAWS (Protection Against Wrapped Sequences)
- **~100行**

---

## 第二阶段：代理与隧道 (N2)

### N2.1 — SOCKS5 代理
- SOCKS5协议（RFC 1928）
- 认证：无认证/用户名密码
- CONNECT命令（TCP代理）
- UDP ASSOCIATE（UDP代理）
- 代理链支持
- **~300行**

### N2.2 — HTTP代理
- HTTP CONNECT隧道
- 正向代理（代理客户端请求）
- 代理认证（Basic/Digest）
- 连接池和keep-alive
- **~250行**

### N2.3 — PPPoE
- PPPoE发现（PADI/PADO/PADR/PADS）
- PPP会话（LCP/IPCP/PAP/CHAP）
- 中国宽带拨号上网
- `pppoe-start`, `pppoe-status`
- **~350行**

---

## 第三阶段：路由协议 (N3)

### N3.1 — OSPF (Open Shortest Path First)
- OSPFv2链路状态路由协议
- Hello/DBD/LSR/LSU/LSAck报文
- SPF (Dijkstra) 最短路径计算
- 区域划分（Area 0骨干区）
- 邻居状态机
- **~400行**

### N3.2 — BGP (Border Gateway Protocol)
- BGP-4外部路由协议
- OPEN/UPDATE/NOTIFICATION/KEEPALIVE报文
- 路径属性：AS_PATH, NEXT_HOP, LOCAL_PREF, MED
- 路由策略（过滤/优先级）
- BGP FSM状态机
- **~450行**

### N3.3 — RIP (Routing Information Protocol)
- RIPv2距离向量路由
- 周期性路由更新（30秒）
- 水平分割、毒性逆转
- 适合小型网络
- **~200行**

---

## 第四阶段：应用协议 (N4)

### N4.1 — gRPC
- Protocol Buffers编解码（简化版）
- HTTP/2上的RPC调用
- 一元/服务端流/客户端流/双向流
- 服务定义和方法注册
- **~350行**

### N4.2 — SMTP（邮件发送）
- SMTP客户端（RFC 5321）
- EHLO/MAIL FROM/RCPT TO/DATA
- STARTTLS加密
- 邮件队列
- **~250行**

### N4.3 — IMAP（邮件接收）
- IMAP4rev1客户端（RFC 3501）
- LOGIN/SELECT/FETCH/SEARCH
- 邮箱管理（INBOX, Sent, Drafts）
- MIME解析（纯文本）
- **~300行**

### N4.4 — TFTP
- TFTP服务端/客户端（RFC 1350）
- RRQ/WRQ/DATA/ACK/ERROR
- 512字节块传输
- PXE网络启动支持
- **~150行**

---

## 第五阶段：QoS与流控 (N5)

### N5.1 — 流量整形 (Traffic Shaping)
- tc/qdisc等价实现
- 令牌桶（Token Bucket）限速
- 带宽限制（per-IP, per-port）
- 突发流量处理
- **~250行**

### N5.2 — 队列调度 (Queueing Disciplines)
- FIFO（先进先出）
- SFQ (Stochastic Fair Queueing)
- HTB (Hierarchical Token Bucket)
- 优先级队列（8级）
- **~300行**

### N5.3 — DSCP/ToS 标记
- DiffServ代码点标记
- 入口分类（按协议/端口/IP）
- 出口标记（设置DSCP值）
- ECN (Explicit Congestion Notification)
- **~150行**

---

## 第六阶段：可编程网络 (N6)

### N6.1 — Raw Socket
- 原始套接字（AF_PACKET等价）
- 发送/接收原始以太网帧
- 发送/接收原始IP包
- `rawsend`, `rawrecv` 命令
- **~200行**

### N6.2 — BPF (Berkeley Packet Filter)
- 经典BPF字节码解释器
- 包过滤虚拟机（32位寄存器）
- 指令集：LD, ST, ALU, JMP, RET
- tcpdump风格过滤表达式编译
- **~350行**

### N6.3 — eBPF (Extended BPF)
- 扩展BPF：64位寄存器, 更多指令
- Map数据结构（HashMap, Array）
- Helper函数（包修改/重定向/丢弃）
- XDP (eXpress Data Path) 快速包处理
- 程序加载和验证
- **~500行**

---

## 第七阶段：高级特性 (N7)

### N7.1 — Bonding/Link Aggregation
- 多网卡绑定（bond0）
- 模式：active-backup, balance-rr, 802.3ad (LACP)
- 故障切换检测
- **~250行**

### N7.2 — IGMP组播
- IGMPv2/v3组播组管理
- 组成员报告/离开
- 组播路由（PIM简化版）
- **~200行**

### N7.3 — RADIUS认证
- RADIUS客户端（RFC 2865）
- Access-Request/Accept/Reject
- 802.1X网络准入控制
- **~250行**

### N7.4 — SNMP网络监控
- SNMPv2c代理
- MIB-II（接口、IP、TCP、UDP统计）
- GET/SET/GETNEXT/TRAP
- 社区字符串认证
- **~300行**

---

## 第八阶段：高性能网络 (N8)

### N8.1 — 25/100GbE 网卡驱动
- Mellanox ConnectX-5/6 (mlx5) 驱动框架
- Intel E810 (ice) 驱动框架
- 多队列（Multi-Queue）收发
- RSS (Receive Side Scaling) — 多核包分发
- 中断合并 (Interrupt Coalescing)
- 巨帧支持 (Jumbo Frame, 9000 MTU)
- **~500行**

### N8.2 — 零拷贝网络 (Zero-Copy)
- `sendfile()` — 文件直接发送到socket，不经过用户态缓冲区
- `splice()` / `tee()` — 管道和socket间零拷贝数据移动
- 页面映射（Page Pinning）— 用户态缓冲区直接映射给DMA
- scatter-gather I/O — 多缓冲区合并发送
- **~300行**

### N8.3 — DPDK风格用户态网络
- 轮询模式驱动（PMD）— 绕过中断，纯轮询收包
- 无锁环形缓冲区（Ring Buffer）— 多生产者多消费者
- 内存池（Mempool）— 预分配包缓冲区，避免malloc
- 批量收发（Burst RX/TX）— 一次处理32/64个包
- 亲和性绑定（Core Pinning）— 网卡队列绑定到特定CPU核
- **~500行**

### N8.4 — AF_XDP
- XDP Socket — 用户态直接收发包
- UMEM共享内存区域
- Fill/Completion/RX/TX环形队列
- 与eBPF XDP程序配合
- **~350行**

### N8.5 — TCP Offload
- Checksum Offload — 校验和由网卡计算
- TSO (TCP Segmentation Offload) — 大包由网卡分段
- GRO (Generic Receive Offload) — 小包由网卡合并
- LRO (Large Receive Offload) — 接收端合并
- **~250行**

---

## 代码量预估

```
现有网络代码: ~8,000 行
N1 TCP增强:   +  550 行 →  8,550
N2 代理隧道:  +  900 行 →  9,450
N3 路由协议:  +1,050 行 → 10,500
N4 应用协议:  +1,050 行 → 11,550
N5 QoS流控:   +  700 行 → 12,250
N6 可编程网络: +1,050 行 → 13,300
N7 高级特性:  +1,000 行 → 14,300
N8 高性能网络: +1,900 行 → 16,200
```

网络子系统总计: **~16,200 行**（占内核总代码 ~15%）

---

## 完成后的网络栈全景

```
┌─────────────────────────────────────────────────────────┐
│ 应用层                                                   │
│ HTTP/1.1 · HTTP/2 · HTTP/3 · gRPC · FTP · TFTP         │
│ SSH · SMTP · IMAP · MQTT · WebSocket · DNS · mDNS      │
│ DHCP服务器 · NTP · SNMP · RADIUS                        │
├─────────────────────────────────────────────────────────┤
│ 代理与VPN                                                │
│ SOCKS5 · HTTP代理 · WireGuard · TLS 1.3 · PPPoE        │
├─────────────────────────────────────────────────────────┤
│ 传输层                                                   │
│ TCP (TFO/SACK/WS/TS + Reno/Cubic/BBR) · QUIC · UDP     │
├─────────────────────────────────────────────────────────┤
│ QoS与流控                                                │
│ Token Bucket · SFQ · HTB · DSCP · ECN · 优先级队列      │
├─────────────────────────────────────────────────────────┤
│ 网络层                                                   │
│ IPv4 · IPv6 · iptables/NAT · OSPF · BGP · RIP          │
│ ICMP · ICMPv6 · NDP · ARP · IGMP                       │
├─────────────────────────────────────────────────────────┤
│ 可编程                                                   │
│ Raw Socket · BPF · eBPF/XDP                              │
├─────────────────────────────────────────────────────────┤
│ 高性能                                                   │
│ 25/100GbE (mlx5/ice) · 零拷贝 · DPDK/PMD · AF_XDP      │
│ TSO/GRO/LRO · RSS · 中断合并 · 巨帧                     │
├─────────────────────────────────────────────────────────┤
│ 链路层                                                   │
│ e1000e · virtio-net · veth · bridge · VLAN 802.1Q       │
│ WiFi 802.11 · Bonding/LACP · PPPoE · 25/100GbE         │
├─────────────────────────────────────────────────────────┤
│ 工具                                                     │
│ ss · nc · ip · traceroute · portscan · ping · tcpdump   │
│ iptables · wg · ntp · dns · dhcp · snmp · rawsend       │
└─────────────────────────────────────────────────────────┘
```

**Born for AI. Built by AI. Connected everywhere.** 🦁
