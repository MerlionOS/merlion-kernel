.PHONY: build run run-serial run-fullscreen run-disk disk test iso run-uefi run-uefi-mac usb clean

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

# -------------------------------------------------------
# UEFI / Real Hardware
# -------------------------------------------------------

ISO_FILE = merlionos.iso

# Build a bootable ISO image using the helper script
iso: build
	tools/make-iso.sh

# Boot the ISO in QEMU with UEFI firmware (Linux — OVMF from distro package)
# Requires OVMF: apt install ovmf, or download from
#   https://github.com/tianocore/tianocore.github.io/wiki/OVMF
run-uefi: iso
	qemu-system-x86_64 \
		-bios /usr/share/OVMF/OVMF_CODE.fd \
		-cdrom $(ISO_FILE) \
		-serial stdio \
		-m 256M

# Boot the ISO in QEMU with UEFI firmware (macOS — Homebrew path)
# Requires: brew install qemu  (ships OVMF firmware)
run-uefi-mac: iso
	@OVMF="$$(find /opt/homebrew/Cellar/qemu /usr/local/Cellar/qemu \
		-name 'edk2-x86_64-code.fd' 2>/dev/null | head -1)"; \
	if [ -z "$$OVMF" ]; then \
		echo "ERROR: OVMF firmware not found."; \
		echo "Install with:  brew install qemu"; \
		echo "Or download from: https://github.com/tianocore/tianocore.github.io/wiki/OVMF"; \
		exit 1; \
	fi; \
	echo "Using OVMF: $$OVMF"; \
	qemu-system-x86_64 \
		-bios "$$OVMF" \
		-cdrom $(ISO_FILE) \
		-serial stdio \
		-m 256M

# Print instructions for writing the ISO to a USB drive
usb: iso
	@echo ""
	@echo "=== Write ISO to USB ==="
	@echo ""
	@echo "1. Identify your USB device (e.g. /dev/sdX on Linux, /dev/diskN on macOS)"
	@echo "   Linux:  lsblk"
	@echo "   macOS:  diskutil list"
	@echo ""
	@echo "2. Unmount the device first"
	@echo "   Linux:  sudo umount /dev/sdX*"
	@echo "   macOS:  diskutil unmountDisk /dev/diskN"
	@echo ""
	@echo "3. Write the ISO (DOUBLE-CHECK the device — this destroys all data!):"
	@echo "   Linux:  sudo dd if=$(ISO_FILE) of=/dev/sdX bs=4M status=progress && sync"
	@echo "   macOS:  sudo dd if=$(ISO_FILE) of=/dev/rdiskN bs=4m && sync"
	@echo ""

clean:
	cargo clean
	rm -f $(DISK_IMG)
