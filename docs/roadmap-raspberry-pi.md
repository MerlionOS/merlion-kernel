[English Version](roadmap-raspberry-pi-en.md)

# MerlionOS Raspberry Pi 移植路线图

> 把 MerlionOS 从 x86_64 移植到 aarch64，在 Raspberry Pi 上运行

---

## 目标硬件

| 型号 | SoC | CPU | RAM | 优先级 |
|------|-----|-----|-----|--------|
| Pi 4B | BCM2711 | 4x Cortex-A72 | 1-8GB | 主要目标 ⭐ |
| Pi 5 | BCM2712 | 4x Cortex-A76 | 4-8GB | 次要目标 |
| Pi 3B+ | BCM2837 | 4x Cortex-A53 | 1GB | 兼容 |
| Pi Zero 2W | BCM2710 | 4x Cortex-A53 | 512MB | 最小化 |

QEMU 测试机器：`qemu-system-aarch64 -machine raspi3b -m 1G`

---

## 第一阶段：串口输出 (P1)

**目标：在QEMU raspi3b上看到 "MerlionOS on Raspberry Pi!"**

### P1.1 — aarch64 启动代码
- 链接脚本：入口地址 `0x80000`（Pi固件加载点）
- `_start` 汇编：读 MPIDR_EL1 只在 core 0 运行，其他核 WFE
- 清零 BSS 段
- 设置栈指针（SP）
- 跳转到 Rust `kernel_main`

### P1.2 — PL011 UART 驱动
- BCM283x PL011 寄存器（MMIO地址 `0xFE201000` / `0x3F201000`）
- `uart_init()`：设置波特率 115200、8N1、使能TX/RX
- `uart_putc(c)`、`uart_puts(s)`
- 实现 `serial_println!` 宏指向 PL011

### P1.3 — 构建系统
- 新 target：`aarch64-unknown-none`
- Cargo.toml：条件依赖（不依赖 `bootloader` crate）
- Makefile：`make pi` 生成 `kernel8.img`
- `config.txt`：`arm_64bit=1`, `enable_uart=1`

**验证**：`qemu-system-aarch64 -machine raspi3b -serial stdio -kernel kernel8.img`

**预估：~500 行新代码**

---

## 第二阶段：中断与定时器 (P2)

**目标：键盘输入、定时器中断**

### P2.1 — 异常向量表
- ARM64 异常向量表（VBAR_EL1）
- 同步异常处理（SVC系统调用）
- IRQ 处理入口

### P2.2 — GIC (Generic Interrupt Controller)
- BCM2711: GICv2（Pi 4）
- BCM2837: 传统中断控制器（Pi 3）
- 中断使能/禁止/确认
- IRQ分发到处理函数

### P2.3 — ARM Generic Timer
- CNTFRQ_EL0 读取频率
- CNTP_TVAL_EL0 设置定时
- 定时器中断触发调度

### P2.4 — Mailbox 通信
- VideoCore mailbox（获取内存信息、MAC地址等）
- 查询 ARM 内存大小
- 获取板子型号/序列号

**预估：~800 行新代码**

---

## 第三阶段：内存管理 (P3)

**目标：堆分配器工作，可以用 alloc**

### P3.1 — ARM 页表
- 4KB granule、4级页表（与x86类似但格式不同）
- 内核映射（高半部分）
- MMIO 区域映射（设备内存、non-cacheable）

### P3.2 — 帧分配器
- 从 mailbox 获取内存大小
- 位图帧分配器
- 或复用现有的 bump allocator

### P3.3 — 堆分配器
- 复用 `linked_list_allocator`
- 映射堆页面
- `#[global_allocator]` 可用

**复用**：`allocator.rs` 大部分可直接复用

**预估：~600 行新代码**

---

## 第四阶段：任务切换 (P4)

**目标：多任务运行，shell可用**

### P4.1 — 上下文切换
- 保存/恢复 callee-saved 寄存器（x19-x30, SP, LR）
- naked 函数实现（类似x86的 context_switch）
- 任务栈分配

### P4.2 — 抢占式调度
- 定时器中断触发 `yield_now()`
- 复用 `task.rs` 的调度逻辑（Round-Robin）

### P4.3 — Shell
- 复用 `shell.rs`（完全不依赖x86）
- UART输入 → KeyEvent → shell
- 所有纯逻辑命令可用（help, ls, cat, vim...）

**复用**：`task.rs` 逻辑复用，只重写汇编部分（~30行）

**预估：~400 行新代码**

---

## 第五阶段：硬件驱动 (P5)

**目标：SD卡、网卡、USB、HDMI**

### P5.1 — EMMC/SD 卡驱动
- BCM283x EMMC 控制器
- SD卡初始化（CMD0, CMD8, ACMD41, CMD2, CMD3）
- 块读写（CMD17/CMD24）
- 挂载 FAT32/ext4 分区

### P5.2 — USB (DWC2)
- Pi 3: DesignWare USB2 OTG 控制器
- Pi 4: xHCI（可部分复用现有 `xhci.rs`）
- USB 键盘（HID）

### P5.3 — 以太网
- Pi 3: USB以太网（SMSC LAN9514）
- Pi 4: 板载 BCM54213PE（RGMII接口）
- MAC/PHY 初始化

### P5.4 — HDMI Framebuffer
- 通过 Mailbox 请求 framebuffer
- 设置分辨率（1920x1080或自动检测）
- 复用 `framebuf.rs` + `widget.rs` 渲染

### P5.5 — GPIO
- 40-pin header GPIO 控制
- 输入/输出/上拉/下拉/中断
- `gpio set <pin> <0|1>`, `gpio read <pin>`
- LED闪烁（Activity LED = GPIO 47）

**预估：~3000 行新代码**

---

## 第六阶段：网络与服务 (P6)

**目标：Pi 可以联网、跑HTTP服务**

### P6.1 — TCP/IP 栈
- 复用 `netstack.rs`、`tcp_real.rs`（完全可复用）
- 以太网驱动 → netstack 后端

### P6.2 — DHCP + DNS
- 复用 `dhcp_client.rs`、`dns_client.rs`
- 自动获取IP

### P6.3 — HTTP 服务器
- 复用 `httpd.rs` + `http_middleware.rs`
- 从Pi上浏览器访问 MerlionOS dashboard

### P6.4 — SSH 服务器
- 复用 `sshd.rs`
- 从电脑 SSH 到 Pi

**复用度：~95%，几乎不需要新代码**

---

## 第七阶段：WiFi 与 IoT (P7)

**目标：Pi 作为 IoT 网关**

### P7.1 — WiFi (BCM43xx)
- Pi 3/4 板载 WiFi（BCM43438/BCM43455）
- SDIO 接口驱动
- WiFi STA 模式（连接路由器）
- WiFi AP 模式（做热点）

### P7.2 — 蓝牙
- BCM43xx 蓝牙（通过 UART/HCI）
- 复用 `bluetooth.rs` HCI层

### P7.3 — I2C/SPI 传感器
- I2C 总线驱动（BCM283x BSC）
- SPI 总线驱动
- 传感器读取（温度、湿度、加速度等）
- `i2c scan`, `i2c read <addr> <reg>`

### P7.4 — MQTT IoT
- 复用 `mqtt_broker.rs`
- 传感器数据 → MQTT → 云端

**预估：~2000 行新代码**

---

## 架构设计

### 代码结构

```
src/
├── arch/
│   ├── x86_64/          # 现有x86代码
│   │   ├── gdt.rs
│   │   ├── idt.rs
│   │   ├── pic.rs
│   │   ├── pit.rs
│   │   ├── apic.rs
│   │   └── context_switch.S
│   └── aarch64/         # 新增ARM代码
│       ├── boot.S        # _start 入口
│       ├── exceptions.rs  # 异常向量表
│       ├── gic.rs         # 中断控制器
│       ├── timer.rs       # ARM Generic Timer
│       ├── uart.rs        # PL011 UART
│       ├── mmu.rs         # ARM 页表
│       ├── mailbox.rs     # VideoCore mailbox
│       ├── gpio.rs        # GPIO
│       ├── emmc.rs        # SD卡
│       └── context_switch.S
├── drivers/
│   ├── bcm2711/          # Pi 4 特有
│   │   ├── eth.rs         # 板载以太网
│   │   └── pcie.rs        # PCIe (Pi 4有)
│   └── bcm43xx/          # WiFi/BT
│       ├── sdio.rs
│       └── wifi.rs
├── kernel/               # 架构无关（现有代码迁移）
│   ├── vfs.rs
│   ├── shell.rs
│   ├── task.rs (逻辑部分)
│   ├── security.rs
│   ├── tcp.rs
│   └── ... (200+ 模块)
└── main.rs               # 条件编译入口
```

### 条件编译

```rust
// main.rs
#[cfg(target_arch = "x86_64")]
mod arch_x86_64;

#[cfg(target_arch = "aarch64")]
mod arch_aarch64;

// 架构无关的代码直接用
use kernel::*;
```

---

## 代码量预估

| 阶段 | 新增代码 | 累计 | 复用x86代码 |
|------|---------|------|------------|
| P1 串口 | ~500行 | 500 | 0 |
| P2 中断 | ~800行 | 1,300 | 0 |
| P3 内存 | ~600行 | 1,900 | ~200行 |
| P4 任务 | ~400行 | 2,300 | ~800行 |
| P5 硬件 | ~3,000行 | 5,300 | ~500行 |
| P6 网络 | ~200行 | 5,500 | ~5,000行 |
| P7 IoT | ~2,000行 | 7,500 | ~1,000行 |
| **总计** | **~7,500行** | | **~7,500行复用** |

最终 MerlionOS (Pi版) ≈ **90,000 行**（82K现有 + 7.5K新增 + 架构重构）

---

## 开发工具

### QEMU 测试（不需要真机）

```bash
# Pi 3
qemu-system-aarch64 \
  -machine raspi3b -m 1G \
  -serial stdio -display none \
  -kernel kernel8.img

# 通用 aarch64（更简单，推荐先用这个）
qemu-system-aarch64 \
  -machine virt -cpu cortex-a72 -m 1G \
  -serial stdio -display none \
  -kernel kernel8.img
```

### 真机部署

```bash
# SD卡结构
/boot/
├── bootcode.bin     # Pi GPU 固件
├── start.elf        # Pi GPU 固件
├── config.txt       # 启动配置
├── kernel8.img      # MerlionOS 内核
└── cmdline.txt      # 内核参数

# config.txt
arm_64bit=1
enable_uart=1
kernel=kernel8.img
```

### 串口调试

```
Pi GPIO14 (TXD) → USB-Serial RX
Pi GPIO15 (RXD) → USB-Serial TX
Pi GND          → USB-Serial GND

screen /dev/tty.usbserial-* 115200
```

---

## 时间线

| 阶段 | 预估时间 | 里程碑 |
|------|---------|--------|
| P1 | 1-2天 | QEMU上看到串口输出 |
| P2 | 2-3天 | 定时器中断工作 |
| P3 | 2-3天 | 堆分配可用 |
| P4 | 1-2天 | Shell可交互 |
| P5 | 1-2周 | SD卡+HDMI+USB |
| P6 | 1-2天 | HTTP/SSH服务器跑起来 |
| P7 | 1-2周 | WiFi+GPIO+IoT |

**P1-P4（基础可用）用AI agents并行开发，可以在一个session完成。**

---

## 愿景

```
MerlionOS on Raspberry Pi:

┌─────────────────────────────────┐
│  MerlionOS v80.0.0 (aarch64)    │
│  4x Cortex-A72 @ 1.5GHz        │
│  RAM: 4096 MiB                  │
│                                  │
│  merlion> help                   │
│  358 commands available          │
│  merlion> ifconfig               │
│  eth0: 192.168.1.50             │
│  wlan0: 192.168.1.51            │
│  merlion> gpio set 17 1         │
│  GPIO 17: HIGH                   │
│  merlion> serve 80              │
│  HTTP server on :80              │
│  merlion> mqtt-stats            │
│  MQTT: 3 clients, 12 topics     │
└─────────────────────────────────┘
```

**Born for AI. Built by AI. Runs everywhere.** 🦁
