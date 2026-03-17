# MerlionOS 性能优化路线图

> 不加新功能，让现有的东西真正好用

---

## 诊断结果

| 问题 | 现状 | 影响 |
|------|------|------|
| shell.rs 巨大 | 3,882行，956个match arms | 维护困难，编译慢 |
| 154个init()调用 | 每个模块都在启动时初始化 | 启动慢，OOM风险 |
| 4MB堆 | 所有模块init时分配 | 浪费内存 |
| 302个模块 | 全部编译进内核 | 二进制大，启动慢 |
| Vim键盘不工作 | Esc/insert切换失败 | 用户体验差 |
| VFS线性搜索 | O(n) inode查找 | 文件多时慢 |
| UEFI启动不完整 | 到shell但未充分测试 | 真机不可靠 |

---

## O1: 启动优化

### O1.1 — 懒加载模块
现在154个`init()`全部在启动时调用。大部分模块（浏览器、邮件、音乐、游戏）不需要在启动时初始化。

方案：分为 **核心模块**（必须启动时加载）和 **延迟模块**（首次使用时加载）。

核心（~30个）：gdt, idt, timer, memory, allocator, task, vfs, serial, keyboard, shell, security, env
延迟（~120个）：browser, email, music_player, snake, tetris, vim, compositor, wifi, bluetooth, ...

预期效果：启动时间减少60%+，初始堆使用减少50%+

### O1.2 — 减小堆占用
分析每个模块init()的堆分配：
- 找出谁分配最多（Vec::new, String, Mutex<Vec<>>）
- 不需要启动时分配的改为lazy static
- 减小预分配大小（如sdcard的模拟缓冲区）

目标：堆从4MB降到1MB以内即可启动

### O1.3 — 启动时间测量
在每个init()前后记录tick，生成启动时间报告：
```
[boot]   0ms gdt::init()
[boot]   1ms timer::init()
[boot]   2ms memory::init()
[boot]  50ms allocator::init()
[boot]  52ms task::init()
...
[boot] 800ms TOTAL
```

---

## O2: Shell重构

### O2.1 — 拆分shell.rs
3,882行、956个match arms的单文件不可维护。拆分为：
- `shell/mod.rs` — 核心（输入处理、历史、管道）
- `shell/dispatch.rs` — 命令分发（大match）
- `shell/help.rs` — help文本
- `shell/builtins.rs` — 内建命令（cd, echo, export等）

### O2.2 — 命令注册表
替代巨大的match block，用哈希表分发：
```rust
static COMMANDS: Mutex<Vec<(&str, fn(&str))>> = ...;
fn register(name: &str, handler: fn(&str));
fn dispatch(cmd: &str) { lookup and call }
```
好处：模块自己注册命令，shell不需要知道所有模块

### O2.3 — Tab补全优化
现有的autocomplete.rs基础上，预建命令名索引，减少补全延迟

---

## O3: 真正的Bug修复

### O3.1 — Vim键盘
问题：在QEMU窗口中按键没有到达vim
原因可能：
- 键盘中断dispatch优先级问题
- QEMU窗口焦点问题
- PS/2扫码转换缺失
修复：加调试日志，逐步定位

### O3.2 — UEFI启动完善
当前状态：QEMU UEFI能启动到"Kernel initialization complete"
需要：
- 加更多网络模块的init
- 测试键盘输入在UEFI模式下是否工作
- 测试framebuffer显示

### O3.3 — 登录流程
当前：密码为空直接Enter就能登录
需要：确保login在各种模式下都能正常工作（VGA、串口、UEFI framebuffer）

---

## O4: 内存优化

### O4.1 — VFS优化
当前O(n)线性搜索inode。改为哈希表查找：
- 路径→inode映射用FNV哈希
- 目录listing仍用Vec但按名排序

### O4.2 — 减少Vec分配
很多模块用 `Vec::new()` 然后push，可以用固定大小数组替代：
- 小配置表用 `[Option<T>; N]` 替代 `Vec<T>`
- 预计可减少数千次小分配

### O4.3 — 栈大小优化
当前每个任务16KB栈。分析实际使用，可能4KB就够

---

## O5: 编译优化

### O5.1 — 条件编译
不是所有模块都需要编译。加feature gates：
```toml
[features]
default = ["core"]
core = []          # 30个核心模块
desktop = ["core"] # +窗口/桌面
network = ["core"] # +完整网络栈
ai = ["core"]      # +AI模块
full = ["desktop", "network", "ai"]  # 全部
```

### O5.2 — 增量编译友好
拆分大文件（shell.rs, vim.rs），避免改一行重编译3000+行

---

## O6: 测试

### O6.1 — 启动冒烟测试
QEMU + 串口断言：启动→登录→执行命令→验证输出→退出
```bash
make test-boot  # 自动化启动测试
```

### O6.2 — 命令回归测试
每个shell命令都应该有基本的输入→预期输出测试

### O6.3 — UEFI启动测试
自动化UEFI启动测试（QEMU + OVMF）

---

## 优先级

| 优先 | 项目 | 效果 |
|------|------|------|
| 🔴 P0 | O3.1 Vim键盘修复 | 用户体验 |
| 🔴 P0 | O1.1 懒加载模块 | 启动速度+内存 |
| 🟡 P1 | O2.1 Shell拆分 | 可维护性 |
| 🟡 P1 | O3.2 UEFI完善 | 真机可用 |
| 🟡 P1 | O6.1 冒烟测试 | 质量保证 |
| 🟢 P2 | O4.1 VFS优化 | 性能 |
| 🟢 P2 | O5.1 条件编译 | 编译时间 |
| 🟢 P2 | O1.3 启动测量 | 可观测 |
