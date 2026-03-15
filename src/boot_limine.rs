/// Limine boot protocol support.
/// Provides request structures and initialization for Limine bootloader.
/// When booting via Limine (UEFI or BIOS), this module handles:
/// - Framebuffer acquisition (GOP on UEFI)
/// - Memory map
/// - RSDP (ACPI) pointer
/// - Higher-half direct map (HHDM) for physical memory access
///
/// Limine protocol: the bootloader scans the kernel ELF for magic
/// request structs and fills in the responses before jumping to _start.

use core::sync::atomic::{AtomicBool, Ordering};

static LIMINE_BOOT: AtomicBool = AtomicBool::new(false);

/// Check if we booted via Limine.
pub fn is_limine_boot() -> bool {
    LIMINE_BOOT.load(Ordering::SeqCst)
}

/// Framebuffer info from Limine.
#[derive(Clone, Copy)]
pub struct LimineFb {
    pub addr: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,  // bytes per row
    pub bpp: u16,    // bits per pixel
}

/// Memory map entry from Limine.
#[derive(Clone, Copy)]
pub struct LimineMemEntry {
    pub base: u64,
    pub length: u64,
    pub mem_type: u32,
}

/// Memory region types (Limine protocol).
pub const LIMINE_MEM_USABLE: u32 = 0;
pub const LIMINE_MEM_RESERVED: u32 = 1;
pub const LIMINE_MEM_ACPI_RECLAIMABLE: u32 = 2;
pub const LIMINE_MEM_ACPI_NVS: u32 = 3;
pub const LIMINE_MEM_BAD: u32 = 4;
pub const LIMINE_MEM_BOOTLOADER: u32 = 5;
pub const LIMINE_MEM_KERNEL: u32 = 6;
pub const LIMINE_MEM_FRAMEBUFFER: u32 = 7;

/// Boot info collected from Limine responses.
pub struct LimineBootInfo {
    pub framebuffer: Option<LimineFb>,
    pub hhdm_offset: u64,    // higher-half direct map offset
    pub rsdp_addr: u64,      // ACPI RSDP physical address
    pub memory_map: alloc::vec::Vec<LimineMemEntry>,
}

/// Initialize from Limine boot info.
/// Called when we detect Limine protocol (future: automatic detection).
pub fn init_from_limine(info: &LimineBootInfo) {
    LIMINE_BOOT.store(true, Ordering::SeqCst);

    // Set up framebuffer console if available
    if let Some(fb) = info.framebuffer {
        let fb_info = crate::fbconsole::FbInfo {
            addr: fb.addr,
            width: fb.width,
            height: fb.height,
            stride: fb.pitch,
            bpp: (fb.bpp / 8) as u8,
        };
        crate::fbconsole::CONSOLE.lock().init(fb_info);
        crate::serial_println!("[limine] framebuffer: {}x{} at {:#x}",
            fb.width, fb.height, fb.addr);
    }

    // Log memory map
    let usable: u64 = info.memory_map.iter()
        .filter(|e| e.mem_type == LIMINE_MEM_USABLE)
        .map(|e| e.length)
        .sum();
    crate::serial_println!("[limine] HHDM offset: {:#x}", info.hhdm_offset);
    crate::serial_println!("[limine] usable memory: {} KiB", usable / 1024);
    crate::serial_println!("[limine] RSDP at: {:#x}", info.rsdp_addr);
}

/// Instructions for building a Limine-bootable image.
pub fn build_instructions() -> &'static str {
    "\
To build a UEFI-bootable MerlionOS image:

1. Install Limine:
   git clone https://github.com/limine-bootloader/limine --branch=v8.x-binary --depth=1
   cd limine && make

2. Build the kernel ELF:
   cargo build --target x86_64-unknown-none

3. Create the boot image:
   mkdir -p iso/boot iso/EFI/BOOT
   cp target/x86_64-unknown-none/debug/merlion-kernel iso/boot/kernel.elf
   cp limine.conf iso/boot/
   cp limine/limine-uefi-cd.bin iso/EFI/BOOT/BOOTX64.EFI
   xorriso -as mkisofs -b limine/limine-bios-cd.bin \\
     -no-emul-boot -boot-info-table \\
     --efi-boot EFI/BOOT/BOOTX64.EFI \\
     -efi-boot-part --efi-boot-image \\
     iso -o merlionos.iso

4. Boot in QEMU (UEFI):
   qemu-system-x86_64 -bios /usr/share/OVMF/OVMF_CODE.fd \\
     -cdrom merlionos.iso -serial stdio

5. Write to USB:
   sudo dd if=merlionos.iso of=/dev/sdX bs=4M status=progress

6. Boot HP laptop:
   - Disable Secure Boot in BIOS
   - Press F9 for Boot Menu → select USB
"
}
