[English Version](real-hardware-roadmap-en.md)

# MerlionOS 真机运行路线图

> 目标：在一台 HP 笔记本上启动 MerlionOS，进入命令行，联网。

## 当前差距分析

| 组件 | QEMU（当前） | 真机（HP 笔记本） | 工作量 |
|------|-------------|-----------------|--------|
| 启动 | BIOS + bootloader 0.9 | UEFI（现代笔记本无传统 BIOS） | 大 |
| 显示 | VGA 文本模式 0xB8000 | UEFI GOP 帧缓冲（像素模式） | 大 |
| 键盘 | PS/2 端口 0x60 | USB HID（内置键盘走 USB） | 大 |
| 存储 | Virtio-blk | AHCI（SATA）或 NVMe | 大 |
| 网络 | Virtio-net | Intel WiFi 或 Realtek 有线网卡 | 极大 |
| 中断 | 8259 PIC | IOAPIC + MSI（从 ACPI 表获取） | 中 |
| 定时器 | PIT 8254 | HPET 或 APIC Timer | 小 |
| 电源 | 硬编码端口 0x604 | 真实 ACPI（解析 FADT 表） | 中 |
| 内存 | bootloader 提供 | UEFI 内存映射 | 小（换引导器后自动） |

## 分阶段路线

### Phase H1: UEFI 启动（最关键）

**为什么**：现代 HP 笔记本默认只有 UEFI，没有传统 BIOS。不换启动方式，根本开不了机。

**做什么**：
```
1. 从 bootloader 0.9 迁移到 bootloader 0.11+（或 Limine）
   - bootloader 0.11+ 原生支持 UEFI
   - 提供 GOP 帧缓冲信息
   - 提供 UEFI 内存映射

2. 或者用 Limine 引导器（更流行，文档更好）
   - Limine Boot Protocol
   - 支持 BIOS 和 UEFI 双启动
   - 自动提供帧缓冲、内存映射、RSDP

3. 创建 UEFI 可启动 USB
   - GPT 分区表
   - EFI System Partition (FAT32)
   - 内核镜像放在 /EFI/BOOT/
```

**预计代码**：~500 行（主要是适配新引导协议）

### Phase H2: 帧缓冲显示（替换 VGA 文本模式）

**为什么**：真机没有 VGA 文本模式（0xB8000 不存在）。UEFI 给的是像素帧缓冲。

**做什么**：
```
1. 从引导器获取帧缓冲地址、分辨率、像素格式
   - 典型：1920×1080, 32bpp, BGR

2. 实现像素级渲染
   - put_pixel(x, y, color)
   - 位图字体渲染（8×16 像素的 PSF/VGA 字体）
   - 字符输出 → 找到字形 → 画像素

3. 替换 VGA Writer
   - println! 宏不变，底层改为帧缓冲渲染
   - 滚动：整块内存复制（memmove）
   - 光标：画一个闪烁的矩形

4. 效果
   - 高分辨率命令行（可能 240 列 × 67 行 在 1080p）
   - 支持更多颜色（RGB 而不是 16 色）
```

**预计代码**：~800 行

### Phase H3: ACPI 表解析

**为什么**：真机的中断路由、CPU 拓扑、电源管理全在 ACPI 表里。

**做什么**：
```
1. 找到 RSDP（Root System Description Pointer）
   - 引导器通常提供 RSDP 地址

2. 解析 RSDT/XSDT → 找到各个表
   - MADT：中断控制器信息（IOAPIC 地址、CPU 列表）
   - FADT：电源管理（关机/重启寄存器）
   - HPET：高精度定时器

3. 初始化 IOAPIC
   - 替换 8259 PIC
   - 配置中断路由（键盘 IRQ1、定时器等）

4. 解析 MADT 获取 CPU 列表
   - 为 SMP 启动做准备
```

**预计代码**：~600 行

### Phase H4: 存储驱动（AHCI 或 NVMe）

**为什么**：HP 笔记本用 SATA SSD（AHCI）或 NVMe SSD，不是 Virtio。

**做什么**：
```
选项 A：AHCI 驱动（SATA，较旧但更简单）
  - PCI 扫描找到 AHCI 控制器（class 01:06）
  - MMIO BAR5 → HBA 内存寄存器
  - 端口初始化、命令列表、FIS 缓冲
  - 构造 SATA 命令（READ DMA EXT / WRITE DMA EXT）
  - ~800 行

选项 B：NVMe 驱动（更现代，HP 新机型）
  - PCI 扫描找到 NVMe 控制器（class 01:08）
  - MMIO BAR0 → NVMe 寄存器
  - Admin Queue + I/O Queue
  - Identify 命令获取磁盘信息
  - Read/Write 命令
  - ~1000 行

建议先做 AHCI（兼容性更好），后续加 NVMe。
```

**预计代码**：~800-1000 行

### Phase H5: USB 主机控制器

**为什么**：笔记本键盘通过 USB 连接（即使是内置键盘）。没有 USB = 没有输入。

> 注：很多笔记本的 UEFI 固件会模拟 PS/2 键盘（USB Legacy Support）。
> 如果开启此选项，我们现有的 PS/2 驱动可能直接能用。
> 但这不可靠，最终需要真正的 USB 驱动。

**做什么**：
```
1. xHCI（USB 3.0）主机控制器驱动
   - PCI 扫描找到 xHCI（class 0C:03:30）
   - MMIO 寄存器映射
   - 命令/传输/事件 Ring 初始化
   - 设备枚举（USB 描述符解析）
   - ~1500 行（USB 驱动是最复杂的部分）

2. USB HID 驱动
   - 键盘 HID 报告解析
   - 按键映射到 KeyEvent
   - ~400 行

3. 可选：USB 大容量存储
   - SCSI over USB (BOT)
   - 用 USB 盘启动/存储
```

**预计代码**：~2000 行（这是最大的单项）

### Phase H6: 网络（真机联网）

**为什么**：这是目标——联网。

**两条路径**：

```
路径 A：USB 以太网适配器（推荐先做，简单得多）
  - 买一个 USB-to-Ethernet 适配器（如 ASIX AX88179）
  - 写 USB CDC-ECM/NCM 驱动
  - 在 USB 基础设施之上，相对简单
  - ~500 行

路径 B：Intel WiFi（iwlwifi，极其复杂）
  - PCIe 设备，需要固件加载
  - 802.11 帧格式、加密（WPA2/WPA3）
  - 扫描、认证、关联、EAPOL 四次握手
  - 这是 Linux 内核里最复杂的驱动之一
  - ~10000+ 行（不推荐在 hobby OS 阶段做）

路径 C：有线网卡（如果笔记本有 RJ45 口或扩展坞）
  - Intel e1000e 或 Realtek RTL8169
  - PCI/PCIe 网卡，相对简单
  - ~800 行
```

**在网卡之上还需要**：
```
- 完整的以太网帧收发（已有基础）
- ARP 解析（已有基础）
- DHCP 客户端（获取 IP 地址）~200 行
- DNS 解析器（查询域名）~200 行
- 完整 TCP（三次握手 + 数据 + 重传）~800 行
- HTTP 客户端（wget）~300 行
```

### Phase H7: 安装与启动

**做什么**：
```
1. 制作可启动 USB
   - 工具脚本：dd 或 limine-deploy
   - GPT + EFI System Partition

2. 从 USB 启动 HP 笔记本
   - 开机按 F9 → Boot Menu → USB
   - 或进 BIOS 设置关闭 Secure Boot

3. 可选：安装到内置 SSD
   - 分区工具
   - UEFI 启动项注册
   - 与 Windows 双启动（后续）
```

## 总工作量估算

| Phase | 内容 | 预计代码量 | 难度 |
|-------|------|-----------|------|
| H1 | UEFI 启动 | ~500 行 | ★★★ |
| H2 | 帧缓冲显示 | ~800 行 | ★★ |
| H3 | ACPI 解析 | ~600 行 | ★★★ |
| H4 | AHCI 存储 | ~800 行 | ★★★★ |
| H5 | USB (xHCI + HID) | ~2000 行 | ★★★★★ |
| H6 | 网络（USB 以太网 + TCP） | ~2000 行 | ★★★★ |
| H7 | 安装工具 | ~200 行 | ★ |
| | **总计** | **~7000 行** | |

当前：11,500 行 → 完成后：~18,500 行

## 推荐顺序

```
H1 (UEFI) → H2 (帧缓冲) → H3 (ACPI) → H7 (USB 启动盘)
    ↓
  此时已经可以在真机上看到命令行了！
  （用 UEFI PS/2 模拟来输入）
    ↓
H4 (AHCI) → H5 (USB) → H6 (网络)
    ↓
  完整的真机体验：启动、存储、键盘、联网
```

## 最快路径（MVP）

如果只想**尽快在真机上看到画面**：

```
1. 换成 Limine 引导器（支持 UEFI）          2 天
2. 写帧缓冲控制台（替换 VGA 文本模式）       3 天
3. 制作 USB 启动盘                           1 天
4. 在 HP 笔记本上启动                        ---

此时你会看到：
  MerlionOS v5.0.0 — Born for AI. Built by AI.
  Booting...
  [ok] GDT loaded
  ...
  merlion>                ← 用 PS/2 模拟输入（如果 BIOS 支持）
```

这个 MVP 大约需要 **1500 行新代码**。

## 注意事项

1. **Secure Boot**：需要在 BIOS 中关闭，否则无法启动未签名的内核
2. **PS/2 模拟**：大多数 UEFI 固件有 "USB Legacy Support"，可以让 PS/2 驱动控制 USB 键盘。但不是所有机器都支持。
3. **显卡**：UEFI GOP 提供的帧缓冲是线性的，不需要显卡驱动。但分辨率可能被锁定在 UEFI 设置的值。
4. **备份**：在真机上测试前，确保 Windows 有备份。建议先用 USB 启动，不要写入内置硬盘。
