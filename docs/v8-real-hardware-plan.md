# v8.0.0 目标：在真实硬件上启动 MerlionOS

## 交付标准

1. ✅ USB 盘插入 HP 笔记本
2. ✅ 开机 F9 选择 USB 启动
3. ✅ 看到 MerlionOS 登录界面（帧缓冲像素渲染）
4. ✅ 能输入命令（PS/2 模拟或 USB 键盘）
5. ✅ `neofetch` 显示真实 CPU 信息

## 技术方案

### 引导器选择：Limine

Limine 是目前最适合 hobby OS 的引导器：
- 同时支持 BIOS 和 UEFI
- 提供帧缓冲、内存映射、RSDP
- 活跃维护，文档清晰
- 不需要 `bootimage` 工具

### 需要改动的文件

```
改动：
  src/main.rs         — 双入口：bootloader 0.9 / Limine
  src/vga.rs          — println! 自动选择 VGA 文本 or 帧缓冲
  src/fbconsole.rs    — 接收 Limine 帧缓冲信息
  src/boot_limine.rs  — 解析 Limine 响应
  Cargo.toml          — 添加 limine 依赖
  Makefile            — 添加 make iso / make usb 目标

新增：
  src/limine_entry.rs — Limine 入口点 + 请求结构
  limine.conf         — 已有，可能需要调整
  tools/make-iso.sh   — 构建可启动 ISO/USB 镜像
```

### 构建流程

```
当前（QEMU BIOS）:
  cargo bootimage → bootimage-merlion-kernel.bin → QEMU

新增（真机 UEFI）:
  cargo build → merlion-kernel (ELF)
             ↓
  Limine + ELF + limine.conf → merlionos.iso
             ↓
  dd if=merlionos.iso of=/dev/sdX → USB 盘
             ↓
  HP 笔记本 F9 → USB 启动 → MerlionOS
```

### println! 自动适配

关键设计：println! 需要在 VGA 文本模式和帧缓冲模式间自动切换：

```rust
pub fn _print(args: fmt::Arguments) {
    if fbconsole::CONSOLE.lock().is_active() {
        // 帧缓冲模式（UEFI/真机）
        fbconsole::CONSOLE.lock().write_fmt(args).unwrap();
    } else {
        // VGA 文本模式（BIOS/QEMU）
        WRITER.lock().write_fmt(args).unwrap();
    }
}
```

这样所有 120+ 命令无需改动，自动适配真机。

### 测试步骤

1. QEMU + OVMF（UEFI 固件）验证
2. 创建 USB 镜像
3. 在真机上测试

### 风险点

- Secure Boot 需要关闭
- 某些 HP 笔记本可能不支持 PS/2 模拟
- 帧缓冲分辨率可能不是预期的
- 内存映射可能和 QEMU 不同
