.PHONY: build run run-serial run-fullscreen run-disk disk test clean

KERNEL_BIN = target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin
DISK_IMG = disk.img

build:
	cargo bootimage

run:
	qemu-system-x86_64 \
		-drive format=raw,file=$(KERNEL_BIN) \
		-serial stdio

run-serial:
	qemu-system-x86_64 \
		-drive format=raw,file=$(KERNEL_BIN) \
		-display none \
		-serial stdio

run-fullscreen:
	qemu-system-x86_64 \
		-drive format=raw,file=$(KERNEL_BIN) \
		-serial stdio \
		-full-screen

# Run with disk + network + LLM proxy (COM2 as PTY)
run-ai: disk
	@echo "Start the LLM proxy in another terminal:"
	@echo "  python3 tools/llm-proxy.py <pty-path> --claude"
	@echo ""
	qemu-system-x86_64 \
		-drive format=raw,file=$(KERNEL_BIN) \
		-drive file=$(DISK_IMG),format=raw,if=virtio \
		-netdev user,id=n0 \
		-device virtio-net-pci,netdev=n0 \
		-serial stdio \
		-serial pty

# Run with disk + network
run-full: disk
	qemu-system-x86_64 \
		-drive format=raw,file=$(KERNEL_BIN) \
		-drive file=$(DISK_IMG),format=raw,if=virtio \
		-netdev user,id=n0 \
		-device virtio-net-pci,netdev=n0 \
		-serial stdio

# Run with a virtio disk attached
run-disk: disk
	qemu-system-x86_64 \
		-drive format=raw,file=$(KERNEL_BIN) \
		-drive file=$(DISK_IMG),format=raw,if=virtio \
		-serial stdio

# Create a 1MB test disk image
disk:
	@test -f $(DISK_IMG) || dd if=/dev/zero of=$(DISK_IMG) bs=1M count=1 2>/dev/null
	@echo "Disk image: $(DISK_IMG) (1 MiB)"

test:
	cargo test --test basic

clean:
	cargo clean
	rm -f $(DISK_IMG)
