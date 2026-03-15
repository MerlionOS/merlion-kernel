[中文版](real-hardware-roadmap.md)

# MerlionOS Real Hardware Roadmap

> Goal: Boot MerlionOS on an HP laptop, reach the command line, and get online.

## Gap Analysis

| Component | QEMU (Current) | Real Hardware (HP Laptop) | Effort |
|-----------|----------------|--------------------------|--------|
| Boot | BIOS + bootloader 0.9 | UEFI (modern laptops lack legacy BIOS) | Large |
| Display | VGA text mode 0xB8000 | UEFI GOP framebuffer (pixel mode) | Large |
| Keyboard | PS/2 port 0x60 | USB HID (built-in keyboard uses USB) | Large |
| Storage | Virtio-blk | AHCI (SATA) or NVMe | Large |
| Network | Virtio-net | Intel WiFi or Realtek wired NIC | Very Large |
| Interrupts | 8259 PIC | IOAPIC + MSI (obtained from ACPI tables) | Medium |
| Timer | PIT 8254 | HPET or APIC Timer | Small |
| Power | Hard-coded port 0x604 | Real ACPI (parse FADT table) | Medium |
| Memory | Provided by bootloader | UEFI memory map | Small (automatic after switching bootloader) |

## Phased Roadmap

### Phase H1: UEFI Boot (Most Critical)

**Why**: Modern HP laptops default to UEFI only, with no legacy BIOS. Without switching the boot method, the machine simply will not start.

**What to do**:
```
1. Migrate from bootloader 0.9 to bootloader 0.11+ (or Limine)
   - bootloader 0.11+ natively supports UEFI
   - Provides GOP framebuffer information
   - Provides UEFI memory map

2. Alternatively, use the Limine bootloader (more popular, better documented)
   - Limine Boot Protocol
   - Supports dual BIOS and UEFI boot
   - Automatically provides framebuffer, memory map, RSDP

3. Create a UEFI-bootable USB
   - GPT partition table
   - EFI System Partition (FAT32)
   - Place kernel image in /EFI/BOOT/
```

**Estimated code**: ~500 lines (mainly adapting to the new boot protocol)

### Phase H2: Framebuffer Display (Replace VGA Text Mode)

**Why**: Real hardware has no VGA text mode (0xB8000 does not exist). UEFI provides a pixel framebuffer instead.

**What to do**:
```
1. Obtain framebuffer address, resolution, and pixel format from the bootloader
   - Typical: 1920x1080, 32bpp, BGR

2. Implement pixel-level rendering
   - put_pixel(x, y, color)
   - Bitmap font rendering (8x16 pixel PSF/VGA fonts)
   - Character output -> find glyph -> draw pixels

3. Replace VGA Writer
   - println! macro remains unchanged; underlying layer switches to framebuffer rendering
   - Scrolling: bulk memory copy (memmove)
   - Cursor: draw a blinking rectangle

4. Result
   - High-resolution command line (potentially 240 columns x 67 rows at 1080p)
   - Support for more colors (RGB instead of 16 colors)
```

**Estimated code**: ~800 lines

### Phase H3: ACPI Table Parsing

**Why**: On real hardware, interrupt routing, CPU topology, and power management are all defined in ACPI tables.

**What to do**:
```
1. Locate the RSDP (Root System Description Pointer)
   - The bootloader typically provides the RSDP address

2. Parse RSDT/XSDT -> find individual tables
   - MADT: interrupt controller info (IOAPIC address, CPU list)
   - FADT: power management (shutdown/reboot registers)
   - HPET: high-precision timer

3. Initialize IOAPIC
   - Replace 8259 PIC
   - Configure interrupt routing (keyboard IRQ1, timer, etc.)

4. Parse MADT to obtain CPU list
   - Preparation for SMP startup
```

**Estimated code**: ~600 lines

### Phase H4: Storage Driver (AHCI or NVMe)

**Why**: HP laptops use SATA SSDs (AHCI) or NVMe SSDs, not Virtio.

**What to do**:
```
Option A: AHCI driver (SATA, older but simpler)
  - PCI scan to find AHCI controller (class 01:06)
  - MMIO BAR5 -> HBA memory registers
  - Port initialization, command list, FIS buffer
  - Construct SATA commands (READ DMA EXT / WRITE DMA EXT)
  - ~800 lines

Option B: NVMe driver (more modern, newer HP models)
  - PCI scan to find NVMe controller (class 01:08)
  - MMIO BAR0 -> NVMe registers
  - Admin Queue + I/O Queue
  - Identify command to retrieve disk information
  - Read/Write commands
  - ~1000 lines

Recommendation: implement AHCI first (better compatibility), add NVMe later.
```

**Estimated code**: ~800-1000 lines

### Phase H5: USB Host Controller

**Why**: The laptop keyboard connects via USB (even the built-in keyboard). No USB = no input.

> Note: Many laptop UEFI firmware implementations emulate a PS/2 keyboard (USB Legacy Support).
> If this option is enabled, our existing PS/2 driver may work directly.
> However, this is unreliable, and a proper USB driver is ultimately needed.

**What to do**:
```
1. xHCI (USB 3.0) host controller driver
   - PCI scan to find xHCI (class 0C:03:30)
   - MMIO register mapping
   - Command/Transfer/Event Ring initialization
   - Device enumeration (USB descriptor parsing)
   - ~1500 lines (the USB driver is the most complex component)

2. USB HID driver
   - Keyboard HID report parsing
   - Key mapping to KeyEvent
   - ~400 lines

3. Optional: USB mass storage
   - SCSI over USB (BOT)
   - Boot/store from USB drives
```

**Estimated code**: ~2000 lines (the largest single item)

### Phase H6: Networking (Real Hardware Connectivity)

**Why**: This is the goal — getting online.

**Two paths**:

```
Path A: USB Ethernet adapter (recommended first, much simpler)
  - Purchase a USB-to-Ethernet adapter (e.g., ASIX AX88179)
  - Write a USB CDC-ECM/NCM driver
  - Builds on top of existing USB infrastructure, relatively straightforward
  - ~500 lines

Path B: Intel WiFi (iwlwifi, extremely complex)
  - PCIe device, requires firmware loading
  - 802.11 frame format, encryption (WPA2/WPA3)
  - Scanning, authentication, association, EAPOL four-way handshake
  - One of the most complex drivers in the Linux kernel
  - ~10000+ lines (not recommended at the hobby OS stage)

Path C: Wired NIC (if the laptop has an RJ45 port or docking station)
  - Intel e1000e or Realtek RTL8169
  - PCI/PCIe NIC, relatively straightforward
  - ~800 lines
```

**On top of the NIC, the following is also needed**:
```
- Complete Ethernet frame send/receive (foundation already exists)
- ARP resolution (foundation already exists)
- DHCP client (obtain IP address) ~200 lines
- DNS resolver (query domain names) ~200 lines
- Full TCP (three-way handshake + data + retransmission) ~800 lines
- HTTP client (wget) ~300 lines
```

### Phase H7: Installation and Boot

**What to do**:
```
1. Create a bootable USB
   - Tooling script: dd or limine-deploy
   - GPT + EFI System Partition

2. Boot the HP laptop from USB
   - Press F9 at power-on -> Boot Menu -> USB
   - Or enter BIOS setup to disable Secure Boot

3. Optional: Install to internal SSD
   - Partitioning tool
   - UEFI boot entry registration
   - Dual-boot with Windows (future work)
```

## Total Effort Estimate

| Phase | Content | Estimated Code | Difficulty |
|-------|---------|---------------|------------|
| H1 | UEFI Boot | ~500 lines | ★★★ |
| H2 | Framebuffer Display | ~800 lines | ★★ |
| H3 | ACPI Parsing | ~600 lines | ★★★ |
| H4 | AHCI Storage | ~800 lines | ★★★★ |
| H5 | USB (xHCI + HID) | ~2000 lines | ★★★★★ |
| H6 | Networking (USB Ethernet + TCP) | ~2000 lines | ★★★★ |
| H7 | Installation Tooling | ~200 lines | ★ |
| | **Total** | **~7000 lines** | |

Current: 11,500 lines -> After completion: ~18,500 lines

## Recommended Order

```
H1 (UEFI) -> H2 (Framebuffer) -> H3 (ACPI) -> H7 (USB Boot Disk)
    |
  At this point you can already see the command line on real hardware!
  (Using UEFI PS/2 emulation for input)
    |
H4 (AHCI) -> H5 (USB) -> H6 (Networking)
    |
  Full real hardware experience: boot, storage, keyboard, networking
```

## Fastest Path (MVP)

If the sole objective is to **see output on real hardware as quickly as possible**:

```
1. Switch to Limine bootloader (UEFI support)           2 days
2. Write framebuffer console (replace VGA text mode)     3 days
3. Create USB boot disk                                  1 day
4. Boot on the HP laptop                                 ---

At that point you will see:
  MerlionOS v5.0.0 — Born for AI. Built by AI.
  Booting...
  [ok] GDT loaded
  ...
  merlion>                <- input via PS/2 emulation (if BIOS supports it)
```

This MVP requires approximately **1500 lines of new code**.

## Important Notes

1. **Secure Boot**: Must be disabled in BIOS settings; otherwise the unsigned kernel cannot boot.
2. **PS/2 Emulation**: Most UEFI firmware includes "USB Legacy Support", which allows the PS/2 driver to control the USB keyboard. However, not all machines support this.
3. **Graphics**: The UEFI GOP framebuffer is linear and does not require a graphics driver. However, the resolution may be locked to the value set in the UEFI configuration.
4. **Backups**: Before testing on real hardware, ensure Windows is backed up. It is recommended to boot from USB first and avoid writing to the internal drive.
