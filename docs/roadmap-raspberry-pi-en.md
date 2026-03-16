[中文版](roadmap-raspberry-pi.md)

# MerlionOS Raspberry Pi Porting Roadmap

> Port MerlionOS from x86_64 to aarch64, running on Raspberry Pi

## Target Hardware

| Model | SoC | CPU | RAM | Priority |
|-------|-----|-----|-----|----------|
| Pi 4B | BCM2711 | 4x Cortex-A72 | 1-8GB | Primary ⭐ |
| Pi 5 | BCM2712 | 4x Cortex-A76 | 4-8GB | Secondary |
| Pi 3B+ | BCM2837 | 4x Cortex-A53 | 1GB | Compatible |

QEMU test: `qemu-system-aarch64 -machine raspi3b -m 1G`

## Phases

### P1: Serial Output (~500 lines)
- aarch64 `_start` at `0x80000`, single-core boot
- PL011 UART driver (MMIO `0xFE201000`)
- Build system: `make pi` → `kernel8.img`
- **Milestone: "MerlionOS on Raspberry Pi!" on serial**

### P2: Interrupts & Timer (~800 lines)
- ARM64 exception vector table (VBAR_EL1)
- GICv2 interrupt controller (Pi 4) / legacy IRQ (Pi 3)
- ARM Generic Timer for scheduling
- VideoCore mailbox (memory info, board revision)

### P3: Memory Management (~600 lines)
- ARM 4-level page tables (4KB granule)
- Frame allocator from mailbox memory info
- Reuse `linked_list_allocator` for heap

### P4: Multitasking & Shell (~400 lines)
- Context switch: save/restore x19-x30, SP, LR
- Reuse `task.rs` scheduling logic
- Reuse `shell.rs` — all 358 commands work immediately

### P5: Hardware Drivers (~3000 lines)
- EMMC/SD card (block read/write, FAT32/ext4 mount)
- USB: DWC2 (Pi 3) / xHCI (Pi 4)
- Ethernet: USB-ETH (Pi 3) / BCM54213 (Pi 4)
- HDMI framebuffer via mailbox
- GPIO: 40-pin header control

### P6: Networking (~200 lines new)
- Reuse TCP/IP stack, DHCP, DNS, HTTP, SSH (~95% reuse)
- Ethernet driver → netstack backend

### P7: WiFi & IoT (~2000 lines)
- BCM43xx WiFi (SDIO) — STA + AP mode
- BCM43xx Bluetooth (UART HCI)
- I2C/SPI sensor bus drivers
- MQTT IoT gateway

## Code Estimate

| Phase | New Code | Reused from x86 |
|-------|----------|-----------------|
| P1-P4 | ~2,300 lines | ~1,000 lines |
| P5-P7 | ~5,200 lines | ~6,500 lines |
| **Total** | **~7,500 lines** | **~7,500 lines** |

Final: MerlionOS (Pi) ≈ 90,000 lines

## Architecture

```
src/
├── arch/x86_64/     # Existing x86 code
├── arch/aarch64/    # New ARM code (boot, GIC, timer, UART, MMU, GPIO)
├── drivers/bcm2711/ # Pi-specific drivers (ethernet, PCIe)
├── drivers/bcm43xx/ # WiFi/Bluetooth
└── kernel/          # Architecture-independent (200+ modules, ~95% reuse)
```

## Quick Start (Development)

```bash
# Test with QEMU (no real Pi needed)
qemu-system-aarch64 -machine virt -cpu cortex-a72 -m 1G \
  -serial stdio -kernel kernel8.img

# Deploy to real Pi
# Copy kernel8.img + config.txt to SD card /boot/
```

**Born for AI. Built by AI. Runs everywhere.** 🦁
