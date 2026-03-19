# Docker on MerlionOS

## Two Modes

MerlionOS supports Docker in two ways:

### Mode 1: Built-in Docker (works now, no install needed)

```sh
merlion> docker run alpine /bin/sh
merlion> docker ps
merlion> docker compose up
```

Built into the kernel in Rust. Zero dependencies.

### Mode 2: Real Docker (Go binary, full compatibility)

```sh
merlion> dockerd --runtime=merlionos &
merlion> docker -H unix:///var/run/docker.sock run ubuntu bash
```

Full Docker Engine running in userspace, using MerlionOS kernel as the backend.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    User Interface                        │
│                                                          │
│  Built-in:  merlion> docker run alpine sh                │
│  Real:      merlion> docker -H /var/run/docker.sock ...  │
└─────────────────────┬────────────────────────────────────┘
                      │
          ┌───────────┴───────────┐
          │                       │
          ▼                       ▼
┌─────────────────┐    ┌──────────────────────┐
│  Built-in CLI   │    │  Real Docker CLI     │
│  (shell_cmds.rs)│    │  (Go binary)         │
│  Direct kernel  │    │  Talks to dockerd    │
│  calls          │    │  via Unix socket     │
└────────┬────────┘    └──────────┬───────────┘
         │                        │
         │               /var/run/docker.sock
         │                        │
         │                        ▼
         │             ┌──────────────────────┐
         │             │  dockerd             │
         │             │  (Go binary)         │
         │             │  Image management    │
         │             │  Networking          │
         │             │  Volume management   │
         │             └──────────┬───────────┘
         │                        │
         │                        ▼
         │             ┌──────────────────────┐
         │             │  containerd          │
         │             │  (Go binary)         │
         │             │  Container lifecycle │
         │             └──────────┬───────────┘
         │                        │
         │                        ▼
         │             ┌──────────────────────┐
         │             │  runc                │
         │             │  OR: merlionos-runc  │◄── our runtime
         │             │  (OCI-compatible)    │
         │             └──────────┬───────────┘
         │                        │
         ▼                        ▼
┌─────────────────────────────────────────────────────────┐
│                MerlionOS Kernel                          │
│                                                          │
│  oci_runtime.rs  — container lifecycle                   │
│  container.rs    — PID namespace isolation                │
│  cgroup.rs       — CPU/memory limits (cgroups v2)        │
│  bridge.rs       — network bridge                        │
│  veth.rs         — virtual ethernet pairs                │
│  userspace.rs    — process creation                      │
│  syscall.rs      — 115+ syscalls                         │
└─────────────────────────────────────────────────────────┘
```

## Built-in Docker Commands

Available now, no installation needed:

```sh
# Container lifecycle
docker run <image> [cmd]        # Run a container
docker stop <name>              # Stop a container
docker kill <name>              # Kill a container
docker rm <name>                # Remove a container
docker exec <name> <cmd>        # Execute in container
docker logs <name>              # View logs
docker ps                       # List containers
docker inspect <name>           # Container details

# Image management
docker images                   # List images
docker pull <image>             # Pull an image
docker rmi <image>              # Remove an image

# Docker Compose
docker compose up [file]        # Deploy from compose file
docker compose ps               # List services
docker compose down             # Stop all containers
docker compose logs [name]      # View logs
docker compose restart          # Restart all services

# System
docker info                     # Runtime info
docker stats                    # Statistics
```

### Built-in Features

| Feature | Status |
|---------|--------|
| Container run/stop/kill/rm | ✅ |
| Image pull/list/remove | ✅ |
| PID namespace isolation | ✅ |
| cgroups v2 (CPU/memory limits) | ✅ |
| Network namespace (veth + bridge) | ✅ |
| Mount namespace (bind mounts) | ✅ |
| Overlay filesystem (base + upper) | ✅ |
| Docker Compose (up/down/ps/logs) | ✅ |
| Container logging | ✅ |
| Health checks | ✅ (via MerlionProxy) |

## Installing Real Docker

### Step 1: Build Docker binaries

Docker consists of several Go binaries. Build with our Go port:

```sh
# Using go-merlionos toolchain
cd go-merlionos
./build.sh  # Build Go with GOOS=merlionos

# Build Docker CLI
git clone --depth 1 https://github.com/docker/cli.git
cd cli
GOOS=merlionos GOARCH=amd64 CGO_ENABLED=0 \
    go build -o docker-merlionos ./cmd/docker

# Build dockerd
git clone --depth 1 https://github.com/moby/moby.git
cd moby
GOOS=merlionos GOARCH=amd64 CGO_ENABLED=0 \
    go build -o dockerd-merlionos ./cmd/dockerd

# Build containerd
git clone --depth 1 https://github.com/containerd/containerd.git
cd containerd
GOOS=merlionos GOARCH=amd64 CGO_ENABLED=0 \
    go build -o containerd-merlionos ./cmd/containerd
```

### Step 2: Build merlionos-runc

Instead of Linux runc (which uses Linux-specific namespaces),
MerlionOS provides its own OCI-compatible runtime:

```sh
# merlionos-runc bridges OCI spec → MerlionOS kernel
# It implements the OCI runtime spec:
#   runc create <container-id>
#   runc start <container-id>
#   runc kill <container-id> <signal>
#   runc delete <container-id>
#   runc state <container-id>

# Built-in to kernel — no separate binary needed.
# Real Docker uses it via: dockerd --runtime=merlionos
```

### Step 3: Run on MerlionOS

```sh
# Boot MerlionOS
make run-full

# In MerlionOS shell:

# Start containerd
merlion> run-user containerd &

# Start dockerd with MerlionOS runtime
merlion> run-user dockerd --runtime=merlionos --containerd=/run/containerd/containerd.sock &

# Use Docker normally
merlion> docker -H unix:///var/run/docker.sock run hello-world
```

## When to Use Which

| Use Case | Built-in | Real Docker |
|----------|----------|-------------|
| Quick testing | ✅ Best | Overkill |
| Development | ✅ Good | Good |
| Production | Good | ✅ Best |
| Docker Hub images | Limited | ✅ Full |
| Dockerfile builds | ❌ No | ✅ Yes |
| Docker Compose | ✅ Basic | ✅ Full |
| Docker Swarm | ❌ No | ✅ Yes |
| Kubernetes (CRI) | ❌ No | ✅ Yes |
| Volume mounts | Basic | ✅ Full |
| Multi-stage builds | ❌ No | ✅ Yes |

### Recommendation

- **Start with built-in** — it works now, zero setup
- **Switch to real Docker** when you need Docker Hub, Dockerfiles, or K8s
- Both use the same kernel — cgroups, namespaces, networking

## OCI Runtime Compatibility

MerlionOS's `oci_runtime.rs` implements the [OCI Runtime Spec](https://github.com/opencontainers/runtime-spec):

| OCI Operation | MerlionOS | runc (Linux) |
|--------------|-----------|-------------|
| `create` | oci_runtime::run() | clone + namespaces |
| `start` | oci_runtime::exec() | exec in namespace |
| `kill` | oci_runtime::kill() | signal to PID |
| `delete` | oci_runtime::rm() | cleanup cgroup |
| `state` | oci_runtime::container_info() | /run/runc state |
| Namespaces: PID | ✅ container.rs | ✅ CLONE_NEWPID |
| Namespaces: NET | ✅ veth.rs + bridge.rs | ✅ CLONE_NEWNET |
| Namespaces: MNT | ✅ bind mounts | ✅ CLONE_NEWNS |
| cgroups v2 | ✅ cgroup.rs | ✅ /sys/fs/cgroup |

## Kernel Syscalls Used by Docker

| Docker Component | Syscalls | MerlionOS |
|-----------------|----------|-----------|
| Container creation | clone, unshare | SYS_CLONE (190) ✅ |
| Process management | fork, exec, waitpid, kill | SYS 110-115 ✅ |
| Networking | socket, bind, listen, accept | SYS 130-136 ✅ |
| Epoll (event loop) | epoll_create/ctl/wait | SYS 230-232 ✅ |
| cgroups | open/write to cgroupfs | SYS 100-102 ✅ |
| Filesystem | mount, chroot, pivot_root | ✅ (simplified) |
| Signals | sigaction, kill | SYS 115, 180 ✅ |
| Pipes | pipe, dup2 | SYS 151-152 ✅ |
| Futex (Go runtime) | futex wait/wake | SYS 241-242 ✅ |
| Memory | mmap, brk | SYS 113, 120 ✅ |

## Related

- [MerlionOS Kernel](https://github.com/MerlionOS/merlion-kernel) — 170K lines, 115 syscalls
- [go-merlionos](https://github.com/MerlionOS/go-merlionos) — Go port for building Docker
- [musl-merlionos](https://github.com/MerlionOS/musl-merlionos) — C library for runc
