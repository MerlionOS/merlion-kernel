/// Limine boot entry point.
/// This module defines the Limine protocol requests as static variables.
/// When booting via Limine, the bootloader finds these requests by scanning
/// the kernel ELF, fills in the responses, and jumps to _limine_start.
///
/// This file is only used when building for Limine (UEFI/real hardware).
/// The bootloader 0.9 path (BIOS/QEMU) uses main.rs entry_point! as before.

// Limine request magic numbers and structures.
// These follow the Limine Boot Protocol specification.

/// Limine request header (common to all requests).
#[repr(C)]
pub struct LimineRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *const (),
}

// Safety: requests are static and read-only after boot
unsafe impl Send for LimineRequest {}
unsafe impl Sync for LimineRequest {}

/// Framebuffer request ID.
const FRAMEBUFFER_ID: [u64; 4] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x9d5827dcd881dd75, 0xa3148604f6fab11b];
/// Memory map request ID.
const MEMMAP_ID: [u64; 4] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x67cf3d9d378a806f, 0xe304acdfc50c3c62];
/// HHDM (Higher Half Direct Map) request ID.
const HHDM_ID: [u64; 4] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x48dcf1cb8ad2b852, 0x63984e959a98244b];
/// RSDP request ID.
const RSDP_ID: [u64; 4] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0xc5e77b6b397e7b43, 0x27637845accdcf3c];

/// Static requests — Limine scans the ELF for these.
#[used]
#[link_section = ".limine_reqs"]
static FRAMEBUFFER_REQ: LimineRequest = LimineRequest {
    id: FRAMEBUFFER_ID, revision: 0, response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_reqs"]
static MEMMAP_REQ: LimineRequest = LimineRequest {
    id: MEMMAP_ID, revision: 0, response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_reqs"]
static HHDM_REQ: LimineRequest = LimineRequest {
    id: HHDM_ID, revision: 0, response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_reqs"]
static RSDP_REQ: LimineRequest = LimineRequest {
    id: RSDP_ID, revision: 0, response: core::ptr::null(),
};

/// Limine framebuffer response.
#[repr(C)]
pub struct LimineFbResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *const *const LimineFbEntry,
}

/// Single framebuffer descriptor.
#[repr(C)]
pub struct LimineFbEntry {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

/// Limine HHDM response.
#[repr(C)]
pub struct LimineHhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

/// Instructions for building a Limine-bootable ISO.
pub fn build_instructions() -> &'static str {
    "\
=== Build MerlionOS for Real Hardware ===

Prerequisites:
  brew install xorriso   # macOS
  # or: apt install xorriso limine   # Linux

Step 1: Clone Limine
  git clone https://github.com/limine-bootloader/limine \\
    --branch=v8.x-binary --depth=1
  cd limine && make
  cd ..

Step 2: Build the kernel
  cargo build --target x86_64-unknown-none --release

Step 3: Create ISO
  mkdir -p iso_root/boot iso_root/EFI/BOOT
  cp target/x86_64-unknown-none/release/merlion-kernel iso_root/boot/kernel.elf
  cp limine.conf iso_root/boot/
  cp limine/limine-bios.sys iso_root/boot/
  cp limine/limine-bios-cd.bin iso_root/boot/
  cp limine/limine-uefi-cd.bin iso_root/boot/
  cp limine/BOOTX64.EFI iso_root/EFI/BOOT/
  cp limine/BOOTIA32.EFI iso_root/EFI/BOOT/

  xorriso -as mkisofs -b boot/limine-bios-cd.bin \\
    -no-emul-boot -boot-load-size 4 -boot-info-table \\
    --efi-boot boot/limine-uefi-cd.bin \\
    -efi-boot-part --efi-boot-image \\
    --protective-msdos-label \\
    iso_root -o merlionos.iso

  limine/limine bios-install merlionos.iso

Step 4: Write to USB
  sudo dd if=merlionos.iso of=/dev/sdX bs=4M status=progress

Step 5: Boot
  - HP laptop: press F9 at boot → select USB
  - Disable Secure Boot in BIOS first
"
}
