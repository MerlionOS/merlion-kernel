#!/usr/bin/env bash
#
# make-iso.sh — Build a bootable MerlionOS ISO image (BIOS + UEFI)
# using the Limine bootloader.
#
set -e

# ── Colours ──────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Colour

info()  { printf "${CYAN}[INFO]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[ OK ]${NC}  %s\n" "$*"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$*"; }
die()   { printf "${RED}[ERR ]${NC}  %s\n" "$*" >&2; exit 1; }

# ── Resolve paths ────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
LIMINE_DIR="$SCRIPT_DIR/limine"
ISO_ROOT="$ROOT_DIR/iso_root"
KERNEL_ELF="$ROOT_DIR/target/x86_64-unknown-none/release/merlion-limine"
ISO_OUT="$ROOT_DIR/merlionos.iso"

# ── 1. Check prerequisites ──────────────────────────────────────────
info "Checking prerequisites..."

command -v xorriso >/dev/null 2>&1 || die "xorriso is not installed. Please install it first."
command -v git     >/dev/null 2>&1 || die "git is not installed. Please install it first."

ok "All prerequisites found."

# ── 2. Clone / update Limine v8.x-binary ────────────────────────────
if [ -d "$LIMINE_DIR" ]; then
    info "Limine directory already exists, pulling latest..."
    git -C "$LIMINE_DIR" pull --quiet
else
    info "Cloning Limine v8.x-binary branch..."
    git clone --branch v8.x-binary --depth 1 \
        https://github.com/limine-bootloader/limine.git "$LIMINE_DIR"
fi

# Build the limine utility if needed
if [ ! -f "$LIMINE_DIR/limine" ]; then
    info "Building limine utility..."
    make -C "$LIMINE_DIR"
fi

ok "Limine is ready."

# ── 3. Build the kernel (Limine binary) ────────────────────────────
info "Building MerlionOS kernel for Limine (x86_64, release)..."
RUSTFLAGS="-C link-arg=-T${ROOT_DIR}/linker-limine.ld -C relocation-model=static" \
  cargo build --manifest-path "$ROOT_DIR/Cargo.toml" \
    --bin merlion-limine --target x86_64-unknown-none --release

[ -f "$KERNEL_ELF" ] || die "Kernel ELF not found at $KERNEL_ELF"
ok "Kernel built successfully."

# ── 4. Create ISO directory structure ────────────────────────────────
info "Preparing ISO root..."
rm -rf "$ISO_ROOT"
mkdir -p "$ISO_ROOT/boot"
mkdir -p "$ISO_ROOT/EFI/BOOT"

# ── 5. Copy kernel ──────────────────────────────────────────────────
cp "$KERNEL_ELF" "$ISO_ROOT/boot/kernel.elf"

# ── 6. Copy limine.conf ─────────────────────────────────────────────
cp "$ROOT_DIR/limine.conf" "$ISO_ROOT/boot/limine.conf"

# ── 7. Copy Limine bootloader files ─────────────────────────────────
cp "$LIMINE_DIR/limine-bios.sys"    "$ISO_ROOT/boot/"
cp "$LIMINE_DIR/limine-bios-cd.bin" "$ISO_ROOT/boot/"
cp "$LIMINE_DIR/BOOTX64.EFI"       "$ISO_ROOT/EFI/BOOT/"
cp "$LIMINE_DIR/BOOTIA32.EFI"      "$ISO_ROOT/EFI/BOOT/"

ok "ISO root populated."

# ── 8. Create ISO with xorriso (BIOS + UEFI) ────────────────────────
info "Creating ISO image..."
xorriso -as mkisofs                                              \
    -b boot/limine-bios-cd.bin                                   \
    -no-emul-boot -boot-load-size 4 -boot-info-table             \
    --efi-boot boot/limine-bios.sys                              \
    -efi-boot-part --efi-boot-image                              \
    --protective-msdos-label                                     \
    "$ISO_ROOT" -o "$ISO_OUT"

ok "ISO created at $ISO_OUT"

# ── 9. Install Limine BIOS stages into the ISO ──────────────────────
info "Installing Limine BIOS boot stages..."
"$LIMINE_DIR/limine" bios-install "$ISO_OUT"

ok "Limine BIOS installed."

# ── 10. Done ─────────────────────────────────────────────────────────
echo ""
printf "${GREEN}==========================================${NC}\n"
printf "${GREEN}  MerlionOS ISO built successfully!${NC}\n"
printf "${GREEN}==========================================${NC}\n"
echo ""
info "Output: $ISO_OUT"
echo ""
info "To test with QEMU:"
echo "    qemu-system-x86_64 -cdrom $ISO_OUT -m 256M"
echo ""
info "To write to a USB drive (replace /dev/sdX):"
echo "    sudo dd if=$ISO_OUT of=/dev/sdX bs=4M status=progress && sync"
echo ""
warn "Double-check the target device — dd will overwrite without confirmation!"
