# MerlionOS Deep Dive Roadmap

> 从 stub 到真正可用——让每个子系统都能真实运行。

## 当前状态

MerlionOS 已完成 77 个开发阶段（70 内核 + 7 AI），拥有 56 个模块和 90+ 命令。
但很多子系统是"概念验证"级别的 stub：

| 子系统 | 当前状态 | 目标状态 |
|--------|---------|---------|
| Virtio-blk | 仅 PCI 检测 | 真实磁盘读写 |
| Virtio-net | 仅 PCI 检测 | 真实以太网帧收发 |
| TCP | 模拟握手 + 环回 | 真实三次握手 + 数据传输 |
| ELF 加载 | 仅解析头 | 从磁盘加载并执行二进制 |
| 文件系统 | RAM 磁盘 + VFS | 持久化到真实磁盘 |
| 用户态 | 嵌入式机器码 | 编译独立的用户二进制 |

## Phase A: 真实 Virtio-blk 磁盘 I/O

### 目标
通过 QEMU 的 virtio-blk 设备读写真实的磁盘镜像文件。

### 技术路径

```
1. PCI BAR0 读取 → 获取 I/O 端口基地址
2. 设备初始化：
   - 写 0 到 Status 寄存器（reset）
   - 设置 ACKNOWLEDGE + DRIVER 位
   - 特性协商
   - 设置 FEATURES_OK
   - 分配 Virtqueue（描述符表 + Available Ring + Used Ring）
   - 设置 DRIVER_OK
3. 提交 I/O 请求：
   - 构造 virtio_blk_req（type=read/write, sector, data）
   - 填入描述符链
   - 通知设备（写 queue_notify 端口）
   - 轮询 Used Ring 等待完成
4. Shell 集成：
   - `diskread <sector>` — 读取并显示一个扇区
   - `diskwrite <sector> <data>` — 写入扇区
```

### QEMU 命令行
```sh
# 创建 1MB 测试磁盘
dd if=/dev/zero of=disk.img bs=1M count=1

# 启动 QEMU（加 virtio 磁盘）
qemu-system-x86_64 \
  -drive format=raw,file=target/.../bootimage-merlion-kernel.bin \
  -drive file=disk.img,format=raw,if=virtio \
  -serial stdio
```

### 关键数据结构

```rust
// Virtio Legacy I/O 端口布局（相对于 BAR0）
const DEVICE_FEATURES: u16 = 0;   // 读取设备特性
const GUEST_FEATURES: u16 = 4;    // 写入客户机特性
const QUEUE_ADDRESS: u16 = 8;     // virtqueue 物理地址
const QUEUE_SIZE: u16 = 12;       // virtqueue 大小
const QUEUE_SELECT: u16 = 14;     // 选择 virtqueue
const QUEUE_NOTIFY: u16 = 16;     // 通知设备
const DEVICE_STATUS: u16 = 18;    // 设备状态
const ISR_STATUS: u16 = 19;       // 中断状态

// Virtqueue 描述符
struct VirtqDesc {
    addr: u64,    // 数据物理地址
    len: u32,     // 数据长度
    flags: u16,   // NEXT, WRITE
    next: u16,    // 链式描述符的下一个
}

// Virtio-blk 请求头
struct VirtioBlkReq {
    type_: u32,   // 0=读, 1=写
    reserved: u32,
    sector: u64,  // 起始扇区号
}
```

### 预期成果
```
merlion> diskread 0
Sector 0: 00 00 00 00 00 00 00 00 ...

merlion> diskwrite 0 "Hello from MerlionOS!"
Written 21 bytes to sector 0

merlion> diskread 0
Sector 0: 48 65 6c 6c 6f 20 66 72 ...
```

## Phase B: 真实 ELF 加载与执行

### 目标
从文件系统加载编译好的 ELF 二进制，映射到用户地址空间，并执行。

### 技术路径

```
1. 在宿主机交叉编译用户程序：
   - 独立的 merlion-user crate
   - target = x86_64-unknown-none
   - 链接脚本指定入口地址（0x400000）
   - 使用 ulib.rs 的 syscall 接口

2. 将编译产物写入磁盘镜像

3. 内核加载流程：
   - 从 VFS/磁盘读取 ELF 文件
   - 解析 ELF header 和 program headers
   - 对每个 PT_LOAD 段：
     - 在用户页表中映射对应虚拟地址
     - 从 ELF 文件复制数据到映射的物理页
   - 设置用户栈
   - iretq 到 ELF entry point

4. 用户程序通过 int 0x80 调用系统服务
```

### 用户程序示例

```rust
// merlion-user/src/main.rs
#![no_std]
#![no_main]

use merlion_ulib::*;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Hello from a real ELF binary!");
    let pid = getpid();
    write("My PID is: ");
    write_num(pid);
    write("\n");
    exit(0);
}
```

### 预期成果
```
merlion> load /disk/hello.elf
Loading ELF: x86_64 executable, entry=0x400000
Mapping 1 LOAD segment...
Entering user mode...

Hello from a real ELF binary!
My PID is: 5

merlion> [process] 'hello.elf' exited with code 0
```

## Phase C: 真实 TCP/IP 网络栈

### 目标
通过 virtio-net 发送和接收真实的以太网帧，实现 ARP + IPv4 + TCP。

### 技术路径

```
层次结构（从下到上）：

┌──────────────────────────┐
│  Application (shell)     │  ping, tcpconn, wget
├──────────────────────────┤
│  TCP                     │  3-way handshake, data, FIN
├──────────────────────────┤
│  UDP / ICMP              │  ping echo, DNS query
├──────────────────────────┤
│  IPv4                    │  路由, 分片, 校验和
├──────────────────────────┤
│  ARP                     │  MAC ↔ IP 地址解析
├──────────────────────────┤
│  Ethernet                │  帧封装/解封装
├──────────────────────────┤
│  Virtio-net Driver       │  virtqueue 收发帧
├──────────────────────────┤
│  PCI / Virtio Transport  │  设备初始化
└──────────────────────────┘
```

### 步骤

```
1. Virtio-net 驱动（类似 virtio-blk）：
   - 两个 virtqueue：RX (接收) + TX (发送)
   - RX: 预分配缓冲区，设备往里写收到的帧
   - TX: 填入要发送的帧，通知设备

2. 以太网帧处理：
   - 解析 dst_mac, src_mac, ethertype
   - ARP 请求/回复
   - IPv4 包解析

3. IPv4 层：
   - 源/目标 IP, TTL, 协议号
   - 校验和计算
   - ICMP echo (ping)

4. TCP 实现：
   - SYN → SYN-ACK → ACK（真实包）
   - 序列号/确认号管理
   - 数据传输 + ACK
   - FIN 关闭

5. 应用层：
   - `wget http://10.0.2.2:8080/` — HTTP GET
   - 简单 HTTP 服务器（监听端口）
```

### QEMU 网络配置
```sh
qemu-system-x86_64 ... \
  -netdev user,id=net0,hostfwd=tcp::5555-:80 \
  -device virtio-net-pci,netdev=net0
```

### 预期成果
```
merlion> ping 10.0.2.2
PING 10.0.2.2...
Reply from 10.0.2.2: seq=0 ttl=64 time=1ms
Reply from 10.0.2.2: seq=1 ttl=64 time=0ms

merlion> wget http://10.0.2.2:8080/
HTTP/1.1 200 OK
Hello from host!
```

## 实施优先级

```
Phase A (Virtio-blk)  ←── 先做这个，是 B 和 C 的基础
    ↓
Phase B (ELF 加载)    ←── 有了磁盘，就能加载真正的二进制
    ↓
Phase C (TCP/IP)      ←── 最复杂，但有了前两个的经验会更顺
```

## 预计代码量

| Phase | 新增代码 | 总计 |
|-------|---------|------|
| A (Virtio-blk) | ~500 行 | ~9100 行 |
| B (ELF 加载) | ~400 行 | ~9500 行 |
| C (TCP/IP) | ~800 行 | ~10300 行 |

完成后 MerlionOS 将达到 **~10000 行 Rust**，从 hobby demo 升级为一个**真正能读写磁盘、加载程序、联网通信**的操作系统。
