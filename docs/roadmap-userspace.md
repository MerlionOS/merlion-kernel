[English Version](roadmap-userspace-en.md)

# MerlionOS 用户态路线图

> 从"全部在内核跑"到"真正的用户态程序"

---

## 为什么需要用户态？

现在MerlionOS的所有代码（游戏、编辑器、浏览器、矿工）都跑在Ring 0内核态。这意味着：
- **一个bug崩全系统** — 用户程序的空指针直接内核panic
- **没有隔离** — 程序可以读写任何内存
- **无法跑标准程序** — Rust/C编译的ELF程序无法执行
- **不是真正的OS** — 只是一个"很大的裸金属程序"

用户态完成后，MerlionOS将能：
- 加载并运行标准Rust `no_std` ELF程序
- 程序崩溃不影响内核
- 多个程序同时运行，互相隔离
- 通过syscall安全地访问内核服务

---

## Phase U1: 最小可行用户态

**目标：一个Rust `no_std`程序在Ring 3打印"Hello from userspace!"**

### U1.1 — Syscall扩展
现有7个syscall不够。扩展到基础的20个：

```
// 文件操作
SYS_OPEN    = 10   // open(path, flags) → fd
SYS_READ    = 11   // read(fd, buf, len) → bytes_read
SYS_CLOSE   = 12   // close(fd)
SYS_STAT    = 13   // stat(path, buf)
SYS_LSEEK   = 14   // lseek(fd, offset, whence)

// 目录操作
SYS_MKDIR   = 20   // mkdir(path, mode)
SYS_UNLINK  = 21   // unlink(path)
SYS_READDIR = 22   // readdir(fd, buf, len)
SYS_CHDIR   = 23   // chdir(path)
SYS_GETCWD  = 24   // getcwd(buf, len)

// 进程
SYS_FORK    = 30   // fork() → pid (简化版：clone当前任务)
SYS_EXEC    = 31   // exec(path, argv, envp)
SYS_WAITPID = 32   // waitpid(pid, status)
SYS_BRK     = 33   // brk(addr) → 调整堆大小

// 内存
SYS_MMAP    = 40   // mmap(addr, len, prot, flags)
SYS_MUNMAP  = 41   // munmap(addr, len)

// 网络
SYS_SOCKET  = 50   // socket(domain, type, protocol) → fd
SYS_CONNECT = 51   // connect(fd, addr, len)
SYS_SENDTO  = 52   // sendto(fd, buf, len, flags, addr)
SYS_RECVFROM= 53   // recvfrom(fd, buf, len, flags, addr)
SYS_BIND    = 54   // bind(fd, addr, len)
SYS_LISTEN  = 55   // listen(fd, backlog)
SYS_ACCEPT  = 56   // accept(fd, addr, len) → fd

// 时间
SYS_TIME    = 60   // time() → epoch seconds
SYS_NANOSLEEP = 61 // nanosleep(seconds, nanos)

// 其他
SYS_IOCTL   = 70   // ioctl(fd, request, arg)
SYS_PIPE    = 71   // pipe(fds[2])
SYS_DUP2    = 72   // dup2(oldfd, newfd)
```

**~500行新代码**

### U1.2 — 用户态ELF加载器完善
已有`elf_exec.rs`和`process.rs`，需要：
- 为用户程序创建独立页表（上半部分映射内核，下半部分映射用户代码）
- 映射.text (RX), .rodata (R), .data (RW), .bss (RW)
- 设置用户栈（8MB at 0x7FFF_FFFF_F000向下增长）
- 设置用户堆（brk从.bss结束开始）
- 通过iretq切换到Ring 3 + 用户代码入口

**~300行新代码**

### U1.3 — 用户态库 (libmerlion)
一个Rust crate供用户程序使用：

```rust
// libmerlion/src/lib.rs
#![no_std]

pub fn write(buf: &[u8]) -> isize {
    syscall2(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64)
}

pub fn exit(code: i32) -> ! {
    syscall1(SYS_EXIT, code as u64);
    loop {}
}

pub fn open(path: &str, flags: u32) -> isize {
    syscall2(SYS_OPEN, path.as_ptr() as u64, flags as u64)
}

fn syscall1(num: u64, arg1: u64) -> isize { ... }
fn syscall2(num: u64, arg1: u64, arg2: u64) -> isize { ... }
fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> isize { ... }

// 简单的println!宏
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {
        // format → write syscall
    };
}
```

**单独的Cargo crate，~200行**

### U1.4 — Hello World用户程序
```rust
// hello.rs
#![no_std]
#![no_main]
extern crate libmerlion;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    libmerlion::write(b"Hello from MerlionOS userspace!\n");
    libmerlion::exit(0);
}
```

编译为独立ELF → 嵌入内核或从VFS加载 → 在Ring 3执行

**~50行**

**预估：~1000行新代码，U1完成后有一个能在用户态跑的"Hello World"**

---

## Phase U2: 文件操作

**目标：用户程序能open/read/write/close文件**

### U2.1 — 文件描述符表（per-process）
- 每个进程维护自己的fd表（最大64个fd）
- fd 0=stdin, 1=stdout, 2=stderr（预分配）
- open返回新fd，close释放fd
- fd → VFS inode映射

### U2.2 — 路径解析
- 用户传入的路径字符串需要从用户空间安全复制到内核
- `copy_from_user(user_ptr, kernel_buf, len)` — 验证用户指针有效性
- 防止用户传入内核地址

### U2.3 — 用户程序: cat
```rust
// cat.rs — 跑在用户态
#![no_std]
#![no_main]
use libmerlion::*;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let fd = open("/proc/version", O_RDONLY);
    let mut buf = [0u8; 256];
    let n = read(fd, &mut buf);
    write(&buf[..n as usize]);
    close(fd);
    exit(0);
}
```

**~400行新代码**

---

## Phase U3: 进程管理

**目标：多个用户程序同时运行**

### U3.1 — fork/exec
- `fork()` — 复制当前进程（CoW页表）
- `exec(path)` — 替换当前进程的地址空间为新ELF
- 子进程继承fd表

### U3.2 — waitpid
- 父进程等待子进程退出
- 获取退出码

### U3.3 — Shell执行外部程序
```
merlion> /bin/hello     # fork + exec hello
merlion> /bin/cat /proc/version  # fork + exec cat
```

**~600行新代码**

---

## Phase U4: 用户态网络

**目标：用户程序能建立TCP连接、发HTTP请求**

### U4.1 — Socket syscalls
- socket(AF_INET, SOCK_STREAM, 0) → fd
- connect(fd, addr, len)
- send/recv through fd
- 内核侧：fd→TCP connection映射

### U4.2 — 用户态HTTP客户端
```rust
// wget.rs — 用户态
let fd = socket(AF_INET, SOCK_STREAM, 0);
connect(fd, &addr);
send(fd, b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n");
let n = recv(fd, &mut buf);
write(&buf[..n]);
close(fd);
```

### U4.3 — 用户态QFC矿工
把 `qfc_miner.rs` 从内核移到用户态：
```rust
// qfc-miner-userspace
let task = http_get(rpc_url, fetch_task_body);
let result = run_inference(&task);
let proof = sign_proof(&result);
http_post(rpc_url, submit_proof_body);
```

**~500行新代码**

---

## Phase U5: 标准C程序支持

**目标：编译C程序在MerlionOS上运行**

### U5.1 — 最小libc
不移植musl（8万行太大），自己实现核心函数：
- `printf`, `puts`, `fprintf`
- `malloc`, `free`（用brk syscall）
- `fopen`, `fread`, `fwrite`, `fclose`
- `memcpy`, `memset`, `strlen`, `strcmp`
- `exit`, `getpid`, `sleep`
- `socket`, `connect`, `send`, `recv`

**~1500行**

### U5.2 — C编译器工具链
- 用交叉编译：`x86_64-unknown-merlion-gcc`（基于GCC/musl target）
- 或者更简单：直接用我们的`self_host.rs`编译简单C子集

### U5.3 — 可以跑的C程序
```c
#include <stdio.h>
int main() {
    printf("Hello from C on MerlionOS!\n");
    return 0;
}
```

**~1500行新代码**

---

## Phase U6: 动态链接

**目标：用户程序能使用共享库**

### U6.1 — 动态链接器 (ld.so)
- 解析ELF PT_INTERP、PT_DYNAMIC
- 加载.so到内存
- 重定位（GOT/PLT）
- 已有`elf_runtime.rs`框架

### U6.2 — libmerlion.so
- 将libmerlion编译为共享库
- 用户程序动态链接而非静态嵌入

**~800行新代码**

---

## 代码量预估

```
U1 最小用户态:     ~1,000 行  → 能跑Hello World
U2 文件操作:       ~  400 行  → 能read/write文件
U3 进程管理:       ~  600 行  → fork/exec/多进程
U4 用户态网络:     ~  500 行  → TCP socket
U5 C程序支持:      ~3,000 行  → printf/malloc/libc
U6 动态链接:       ~  800 行  → 共享库
━━━━━━━━━━━━━━━━━━━━━━━━━━━━
总计:              ~6,300 行
```

---

## 里程碑

| Phase | 里程碑 | 验证方法 |
|-------|--------|---------|
| U1 | "Hello from userspace!" | 串口看到Ring 3程序输出 |
| U2 | `cat /proc/version` 在用户态 | 用户程序读取VFS文件 |
| U3 | Shell跑外部程序 | `merlion> /bin/hello` |
| U4 | 用户态HTTP请求 | wget在用户态获取网页 |
| U5 | C程序运行 | `printf("Hello")` 工作 |
| U6 | 动态链接工作 | 用户程序链接libmerlion.so |

---

## 当前基础

已经有的（可以复用的代码）：

| 模块 | 状态 | 用途 |
|------|------|------|
| `process.rs` | ✅ 有 | 用户态页表、Ring 3切换 |
| `elf_exec.rs` | ✅ 有 | ELF加载、PT_LOAD映射 |
| `elf_runtime.rs` | ✅ 有 | 动态链接框架 |
| `syscall.rs` | ⚠️ 7个 | 需要扩展到30+ |
| `vfs.rs` | ✅ 有 | 文件系统（需要加fd表） |
| `fd.rs` | ✅ 有 | 文件描述符（需要per-process） |
| `libc.rs` | ✅ 有 | C标准函数（在内核态，需要移到用户态） |

**不需要从零开始——大部分基础设施已经有了，主要是把它们连接起来。**

---

## 优先级

```
🔴 P0: U1 — 最小用户态（Hello World）
🔴 P0: U2 — 文件操作（能用才有意义）
🟡 P1: U3 — 进程管理（Shell跑程序）
🟡 P1: U4 — 网络（QFC矿工移到用户态）
🟢 P2: U5 — C支持
🟢 P2: U6 — 动态链接
```

**U1可以在一个session完成——大部分是扩展现有的syscall和process模块。**

---

**Born for AI. Built by AI. Now runs user programs.** 🦁
