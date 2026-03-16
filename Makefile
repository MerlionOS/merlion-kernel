.PHONY: build run run-serial run-fullscreen run-disk disk test limine-kernel iso run-uefi run-uefi-mac usb pi pi-img run-pi run-pi-virt pi-sd clean

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
# UEFI / Real Hardware (Limine boot)
# -------------------------------------------------------

LIMINE_ELF = target/x86_64-unknown-none/release/merlion-limine
ISO_FILE = merlionos.iso

# Build kernel ELF for Limine boot (separate binary with _start entry point)
limine-kernel:
	RUSTFLAGS="-C link-arg=-T$(CURDIR)/linker-limine.ld -C relocation-model=static" \
	cargo build --bin merlion-limine --target x86_64-unknown-none --release
	@echo "Kernel ELF: $(LIMINE_ELF)"

# Build a bootable ISO image using the helper script
iso: limine-kernel
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

# -------------------------------------------------------
# Raspberry Pi (aarch64)
# -------------------------------------------------------

PI_KERNEL = target/aarch64-unknown-none/release/merlion-pi

# Build kernel for Raspberry Pi
pi:
	cargo build --bin merlion-pi --target aarch64-unknown-none --release
	@echo "Pi kernel: $(PI_KERNEL)"

# Create kernel8.img for Pi SD card
pi-img: pi
	cp $(PI_KERNEL) kernel8.img
	@echo "Created kernel8.img — copy to Pi SD card /boot/"

# Test Pi kernel in QEMU
run-pi: pi
	qemu-system-aarch64 \
		-machine raspi3b \
		-serial stdio \
		-display none \
		-kernel $(PI_KERNEL)

# Alternative: test with generic virt machine (simpler, more reliable)
run-pi-virt: pi
	qemu-system-aarch64 \
		-machine virt -cpu cortex-a72 -m 1G \
		-serial stdio \
		-display none \
		-kernel $(PI_KERNEL)

# Create Pi SD card image
pi-sd: pi-img
	@echo "=== Prepare SD Card ==="
	@echo "1. Format SD card as FAT32"
	@echo "2. Download Pi firmware:"
	@echo "   https://github.com/raspberrypi/firmware/tree/master/boot"
	@echo "   Copy: bootcode.bin, start.elf, fixup.dat"
	@echo "3. Copy to SD card:"
	@echo "   cp kernel8.img /Volumes/boot/"
	@echo "   cp pi-config.txt /Volumes/boot/config.txt"
	@echo "4. Insert SD card in Pi and power on"
	@echo "5. Connect serial cable to GPIO 14/15"

clean:
	cargo clean
	rm -f $(DISK_IMG)
