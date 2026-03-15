# Contributing to MerlionOS

> **Born for AI. Built by AI.** — but humans are welcome too!

## Quick Start

```sh
# Prerequisites
rustup install nightly
rustup component add rust-src llvm-tools --toolchain nightly
cargo install bootimage
brew install qemu  # macOS (or apt install qemu-system-x86 on Linux)

# Build and run
cd merlion-kernel
make build
make run            # VGA window + serial
make run-fullscreen # immersive mode
make run-full       # with virtio disk + network
make run-ai         # with LLM proxy support
```

## Project Structure

```
merlion-kernel/
├── src/               # 58 kernel modules (~9700 lines of Rust)
├── docs/              # Architecture, AI Native OS, Deep Dive docs
├── tools/             # LLM proxy and utilities
├── .github/workflows/ # CI: auto build + boot test
├── CLAUDE.md          # AI assistant context
├── Cargo.toml         # Rust package manifest
├── Makefile           # Build/run shortcuts
└── README.md          # Project documentation
```

## How to Contribute

### Adding a Shell Command

1. Add the command in `src/shell.rs` → `dispatch()` match block
2. Add help text in the `"help"` case
3. Test: `make run` → type your command

### Adding a Kernel Module

1. Create `src/your_module.rs`
2. Add `pub mod your_module;` to `src/lib.rs`
3. Initialize in `src/main.rs` if needed

### Adding a /proc File

1. Add a `NodeType` variant in `src/vfs.rs`
2. Create the inode in `Filesystem::new()`
3. Handle the read in `read_file()`

### Adding a Loadable Module

1. Create a struct implementing `KernelModule` trait in `src/module.rs`
2. Register it in `module::init()`
3. Users can load/unload via `modprobe`/`rmmod`

### Adding an AI Agent

1. Create a struct implementing `Agent` trait in `src/agent.rs`
2. Register it in `agent::init()`
3. Users interact via `ask <agent_name> <message>`

## Code Style

- **Rust nightly** with `no_std`
- Keep modules focused and small
- Add concise comments for non-obvious low-level code
- Use `serial_println!` for debug output
- Use `klog_println!` for the kernel ring buffer
- Use `println!` for VGA console output
- Prefer clarity over cleverness

## Testing

```sh
make build    # must compile cleanly
make run      # must boot to shell prompt
```

Type `test` in the shell to run 15 built-in kernel self-tests.
Type `demo` to run the full system showcase.

CI automatically builds and boot-tests every push to `main`.

## Architecture

MerlionOS is a **monolithic kernel** — see [docs/architecture.md](docs/architecture.md).

Key design decisions:
- `bootloader` crate v0.9 with `map_physical_memory`
- Static relocation model (required for bootloader 0.9)
- Syscalls via `int 0x80` with naked trampoline
- Preemptive round-robin scheduling
- Per-process page tables for user-mode isolation

## License

This is a hobby/educational project. Feel free to learn from it.
