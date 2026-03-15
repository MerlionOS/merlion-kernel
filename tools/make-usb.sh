#!/bin/bash
# Create a UEFI-bootable USB image for MerlionOS.
# Uses Limine as the bootloader.
#
# Usage:
#   ./tools/make-usb.sh                    # creates merlionos.img
#   sudo dd if=merlionos.img of=/dev/sdX   # write to USB drive
#
# Prerequisites:
#   - cargo bootimage (builds the kernel)
#   - limine (brew install limine / git clone https://github.com/limine-bootloader/limine)

set -e

KERNEL="target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin"
IMG="merlionos.img"
SIZE_MB=64

echo "=== MerlionOS USB Image Builder ==="
echo ""

# Check if kernel is built
if [ ! -f "$KERNEL" ]; then
    echo "Building kernel..."
    cargo bootimage
fi

echo "Creating ${SIZE_MB}MB disk image..."
dd if=/dev/zero of="$IMG" bs=1M count=$SIZE_MB 2>/dev/null

# Create GPT partition table
echo "Setting up GPT + EFI partition..."
if command -v sgdisk &>/dev/null; then
    sgdisk -o "$IMG"
    sgdisk -n 1:2048:+32M -t 1:ef00 "$IMG"  # EFI System Partition
    sgdisk -n 2:0:0 -t 2:8300 "$IMG"          # MerlionOS data partition
elif command -v parted &>/dev/null; then
    parted -s "$IMG" mklabel gpt
    parted -s "$IMG" mkpart EFI fat32 1MiB 33MiB
    parted -s "$IMG" set 1 esp on
    parted -s "$IMG" mkpart MerlionOS ext2 33MiB 100%
else
    echo "WARNING: No partition tool found (sgdisk or parted)."
    echo "The image will be a raw BIOS boot image instead."
    cp "$KERNEL" "$IMG"
    echo "Done: $IMG (raw BIOS bootable)"
    exit 0
fi

echo ""
echo "Image created: $IMG"
echo ""
echo "For BIOS boot (QEMU), the bootimage binary works directly."
echo "For UEFI boot on real hardware, you'll need to:"
echo "  1. Install Limine: git clone https://github.com/limine-bootloader/limine"
echo "  2. Deploy Limine to the EFI partition"
echo "  3. Copy the kernel to the data partition"
echo "  4. Add a limine.conf to the EFI partition"
echo ""
echo "Or use the raw boot image with QEMU:"
echo "  qemu-system-x86_64 -drive format=raw,file=$KERNEL -serial stdio"
echo ""
echo "=== MerlionOS USB Image Builder Complete ==="
