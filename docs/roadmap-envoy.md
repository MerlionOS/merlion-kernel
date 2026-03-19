# Roadmap: Running Envoy on MerlionOS

> 从 "epoll 还没有" 到 "Envoy 跑起来"

## 当前状态 (v101)

### ✅ 已完成

| 组件 | 状态 | 备注 |
|------|------|------|
| epoll | ✅ 已实现 | epoll_create/ctl/wait，16 实例，64 fd/实例 |
| pthreads mutex | ✅ 已实现 | create/lock/unlock/destroy，spin-yield |
| pthreads condvar | ✅ 已实现 | create/wait/signal/broadcast |
| pthreads rwlock | ✅ 已实现 | rdlock/wrlock/unlock |
| futex | ✅ 已实现 | wait/wake，用户态快速锁 |
| fcntl | ✅ 已实现 | F_GETFL/F_SETFL/O_NONBLOCK |
| setsockopt | ✅ 已实现 | SO_REUSEADDR/TCP_NODELAY 等 |
| TCP/IP | ✅ 完整 | Reno/Cubic/BBR 拥塞控制 |
| TLS | ✅ 有 | AES-128, RSA, X.509, DH key exchange |
| HTTP/1.1 | ✅ 有 | 客户端 + 服务器 |
| HTTP/2 | ✅ 有 | http2.rs |
| HTTP/3 + QUIC | ✅ 有 | quic.rs + http3.rs |
| gRPC | ✅ 有 | grpc.rs |
| 反向代理 | ✅ 有 | http_proxy.rs + https_server.rs |
| 负载均衡 | ✅ 部分 | http_middleware.rs |
| iptables/NAT | ✅ 有 | iptables.rs |
| eBPF | ✅ 有 | ebpf.rs |
| Docker/OCI | ✅ 有 | oci_runtime.rs + container.rs |
| 79→95 syscalls | ✅ 有 | 包含 epoll/pthread/fcntl/sockopt |

### ❌ 还需要做的

## Phase E1: musl libc 核心子集 (~2000行)

Envoy 链接 libc，需要以下核心函数在用户态可用：

```
文件 I/O:        fopen, fclose, fread, fwrite, fprintf, fseek, ftell
字符串:          snprintf, sscanf, strncat, strncpy, strrchr, strtoul
内存:            calloc, realloc, memalign, posix_memalign
进程:            getenv, setenv, atexit, abort
时间:            gettimeofday, clock_gettime(CLOCK_MONOTONIC)
网络:            getaddrinfo, freeaddrinfo, inet_ntop, inet_pton
                 htons, ntohs, htonl, ntohl
线程:            pthread_create, pthread_join, pthread_detach
                 pthread_key_create, pthread_setspecific, pthread_getspecific
错误:            strerror_r, errno (thread-local)
```

**工作量: ~2000行 | 优先级: P0**

## Phase E2: 完整 Socket API (~500行)

```
accept4(fd, addr, len, flags)     — 带 SOCK_CLOEXEC/SOCK_NONBLOCK
sendmsg/recvmsg                   — scatter-gather + ancillary data
socketpair                        — Unix domain sockets
poll/ppoll                        — 除了 epoll 外的备选
shutdown(fd, how)                 — 半关闭 TCP
```

**工作量: ~500行 | 优先级: P0**

## Phase E3: eventfd + timerfd (~300行)

Envoy 的事件循环依赖这些 Linux 特有的 fd 类型：

```
eventfd(initval, flags) → fd      — 事件通知
eventfd_read/write                — 计数器语义
timerfd_create(clockid, flags)    — 定时器 fd
timerfd_settime(fd, flags, spec)  — 设置超时
timerfd_gettime(fd)               — 查询剩余时间
```

**工作量: ~300行 | 优先级: P1**

## Phase E4: 完整 ELF 动态链接 (~800行)

Envoy 是动态链接的 C++ 程序，需要：

```
ld-linux.so 完整实现:
  - PT_INTERP 处理
  - .dynamic 段解析（已有 elf_dyn.rs）
  - GOT/PLT 重定位（已有 elf_runtime.rs）
  - RPATH/RUNPATH 搜索
  - LD_LIBRARY_PATH
  - 延迟绑定
  - TLS (__thread) 初始化
```

**工作量: ~800行 | 优先级: P1**

## Phase E5: C++ 运行时支持 (~500行)

```
libstdc++ / libc++ 最小子集:
  - new/delete 操作符
  - __cxa_atexit, __cxa_throw, __cxa_begin_catch
  - type_info, dynamic_cast (RTTI)
  - std::string, std::vector 基础
  - std::mutex, std::thread (映射到 pthread)
  - 异常处理 (unwind)
```

**工作量: ~500行 | 优先级: P2（可以先用 -fno-exceptions 编译）**

## Phase E6: /proc + /sys 完善 (~300行)

Envoy 读取系统信息：

```
/proc/self/maps        — 内存映射（已有 procfs）
/proc/cpuinfo          — CPU 信息（已有）
/proc/self/fd/         — fd 列表
/sys/devices/system/cpu/online  — CPU 拓扑
```

**工作量: ~300行 | 优先级: P2**

## Phase E7: 交叉编译 Envoy (~外部工作)

```
1. 安装 musl 交叉编译工具链:
   x86_64-linux-musl-g++ (静态链接，避免 glibc 依赖)

2. 配置 Envoy 构建:
   bazel build --config=linux-x86_64-musl \
     --define=wasm=disabled \
     --define=tcmalloc=disabled \
     //source/exe:envoy-static

3. 简化版: 先编译 Envoy 的核心组件:
   - connection manager
   - HTTP codec
   - cluster manager
   - router filter

4. 生成静态链接 ELF → 放到 MerlionOS VFS → run-user envoy
```

## 总结

```
Phase    工作量    累计      能跑什么
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
E1       2000行   2000行    简单 C 网络服务 (nginx-lite)
E2        500行   2500行    C socket 服务器
E3        300行   2800行    事件驱动服务器 (libevent 风格)
E4        800行   3600行    动态链接 C 程序
E5        500行   4100行    C++ 程序 (简化版 Envoy)
E6        300行   4400行    完整系统信息
E7       外部      —        编译真正的 Envoy
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
总计:    ~4400行新代码 + Envoy 交叉编译
```

**最短路径**: E1 + E2 + E3 ≈ 2800行 → 能跑事件驱动的 C 网络代理。
这足够跑一个简化版的 Envoy（纯 C 实现的 L7 代理）。

真正的 Envoy（C++，150万行）需要 E4 + E5，但我们可以先做一个
**MerlionProxy**——功能等价的内核级 L7 代理，用已有的
http_proxy + grpc + tls + iptables 模块。
