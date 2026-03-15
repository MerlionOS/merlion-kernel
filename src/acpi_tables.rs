/// ACPI table discovery and parsing for MerlionOS.
///
/// Parses RSDP, RSDT/XSDT, MADT, and FADT tables from firmware memory.
/// Extracts Local APIC entries, I/O APIC addresses, and shutdown/reboot
/// register information. Uses a physical-to-virtual offset provided by
/// the bootloader's `map_physical_memory` feature.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem;
use core::ptr;

/// ACPI 1.0 RSDP structure (20 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Rsdp {
    /// Must be `b"RSD PTR "`.
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    /// 0 = ACPI 1.0 (RSDT), 2 = ACPI 2.0+ (XSDT).
    pub revision: u8,
    /// Physical address of the RSDT.
    pub rsdt_address: u32,
}

/// Extended RSDP fields present when `revision >= 2` (total 36 bytes).
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct RsdpExtended {
    pub base: Rsdp,
    pub length: u32,
    /// Physical address of the XSDT (64-bit).
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

/// Standard header shared by all ACPI System Description Tables.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl SdtHeader {
    /// Return the four-byte signature as a UTF-8 string (lossy).
    pub fn signature_str(&self) -> String {
        let sig = self.signature;
        String::from_utf8_lossy(&sig).into_owned()
    }
}

/// Parsed Local APIC entry from the MADT.
#[derive(Debug, Clone)]
pub struct LocalApicEntry {
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    /// Bit 0: processor enabled. Bit 1: online-capable.
    pub flags: u32,
}

/// Parsed I/O APIC entry from the MADT.
#[derive(Debug, Clone)]
pub struct IoApicEntry {
    pub io_apic_id: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

/// Parsed contents of the MADT (Multiple APIC Description Table).
#[derive(Debug, Clone)]
pub struct Madt {
    pub local_apic_address: u32,
    pub flags: u32,
    pub local_apics: Vec<LocalApicEntry>,
    pub io_apics: Vec<IoApicEntry>,
}

/// Subset of the FADT (Fixed ACPI Description Table) for shutdown/reboot.
#[derive(Debug, Clone)]
pub struct Fadt {
    /// Physical address of the DSDT.
    pub dsdt_address: u64,
    /// PM1a control block I/O port address.
    pub pm1a_cnt_blk: u32,
    /// PM1b control block I/O port address (0 if unused).
    pub pm1b_cnt_blk: u32,
    /// SCI interrupt number.
    pub sci_interrupt: u16,
    /// SMI command port.
    pub smi_cmd: u32,
    /// Value to write to SMI_CMD to enable ACPI.
    pub acpi_enable: u8,
    /// Value to write to SMI_CMD to disable ACPI.
    pub acpi_disable: u8,
    /// Reset register address (from Generic Address Structure).
    pub reset_reg_address: u64,
    /// Value to write to the reset register to trigger a reset.
    pub reset_value: u8,
}

/// Collected ACPI information discovered from firmware tables.
#[derive(Debug, Clone)]
pub struct AcpiInfo {
    pub rsdp_revision: u8,
    pub oem_id: String,
    /// Signatures of all tables found in the RSDT/XSDT.
    pub table_signatures: Vec<String>,
    pub madt: Option<Madt>,
    pub fadt: Option<Fadt>,
}

/// Validate a byte-range checksum (all bytes must sum to zero mod 256).
fn checksum_valid(ptr: *const u8, len: usize) -> bool {
    let mut sum: u8 = 0;
    for i in 0..len {
        sum = sum.wrapping_add(unsafe { ptr::read(ptr.add(i)) });
    }
    sum == 0
}

/// Parse the MADT given a virtual pointer to its SDT header.
fn parse_madt(virt: *const u8) -> Madt {
    let header = unsafe { ptr::read_unaligned(virt as *const SdtHeader) };
    let total_len = header.length as usize;
    let base_offset = mem::size_of::<SdtHeader>() + 8; // +4 LAPIC addr +4 flags

    let local_apic_address = unsafe { ptr::read_unaligned(virt.add(36) as *const u32) };
    let flags = unsafe { ptr::read_unaligned(virt.add(40) as *const u32) };

    let mut local_apics = Vec::new();
    let mut io_apics = Vec::new();
    let mut offset = base_offset;

    while offset + 2 <= total_len {
        let entry_type = unsafe { ptr::read(virt.add(offset)) };
        let entry_len = unsafe { ptr::read(virt.add(offset + 1)) } as usize;
        if entry_len == 0 { break; }

        match entry_type {
            0 if entry_len >= 8 => {
                // Type 0: Processor Local APIC
                local_apics.push(LocalApicEntry {
                    acpi_processor_id: unsafe { ptr::read(virt.add(offset + 2)) },
                    apic_id: unsafe { ptr::read(virt.add(offset + 3)) },
                    flags: unsafe { ptr::read_unaligned(virt.add(offset + 4) as *const u32) },
                });
            }
            1 if entry_len >= 12 => {
                // Type 1: I/O APIC
                io_apics.push(IoApicEntry {
                    io_apic_id: unsafe { ptr::read(virt.add(offset + 2)) },
                    io_apic_address: unsafe { ptr::read_unaligned(virt.add(offset + 4) as *const u32) },
                    global_system_interrupt_base: unsafe { ptr::read_unaligned(virt.add(offset + 8) as *const u32) },
                });
            }
            _ => {} // Interrupt source overrides, NMI, etc.
        }
        offset += entry_len;
    }

    Madt { local_apic_address, flags, local_apics, io_apics }
}

/// Parse the FADT given a virtual pointer to its SDT header.
fn parse_fadt(virt: *const u8) -> Fadt {
    let header = unsafe { ptr::read_unaligned(virt as *const SdtHeader) };
    let len = header.length as usize;

    // Safely read at an offset, returning zero if out of bounds.
    macro_rules! rd {
        ($off:expr, $ty:ty) => {
            if $off + mem::size_of::<$ty>() <= len {
                unsafe { ptr::read_unaligned(virt.add($off) as *const $ty) }
            } else { 0 as $ty }
        };
    }

    let dsdt32 = rd!(40, u32) as u64;
    let dsdt64 = rd!(140, u64);
    Fadt {
        dsdt_address: if dsdt64 != 0 { dsdt64 } else { dsdt32 },
        sci_interrupt: rd!(46, u16),
        smi_cmd: rd!(48, u32),
        acpi_enable: rd!(52, u8),
        acpi_disable: rd!(53, u8),
        pm1a_cnt_blk: rd!(64, u32),
        pm1b_cnt_blk: rd!(68, u32),
        reset_reg_address: rd!(124, u64), // GAS at offset 116, addr at +8
        reset_value: rd!(128, u8),
    }
}

/// Parse ACPI tables starting from the physical address of the RSDP.
///
/// `rsdp_phys` is the physical address of the RSDP (found by the bootloader
/// or by scanning the EBDA / BIOS ROM area). `phys_offset` is the virtual
/// base where all physical memory is identity-mapped by the bootloader, so
/// `virtual = physical + phys_offset`.
///
/// Returns `None` if the RSDP signature or any checksum is invalid.
pub fn parse_from_rsdp(rsdp_phys: u64, phys_offset: u64) -> Option<AcpiInfo> {
    let rsdp_virt = (rsdp_phys + phys_offset) as *const u8;

    // Validate RSDP signature and checksum.
    let rsdp = unsafe { ptr::read_unaligned(rsdp_virt as *const Rsdp) };
    if &rsdp.signature != b"RSD PTR " { return None; }
    if !checksum_valid(rsdp_virt, 20) { return None; }

    let oem_id: String = String::from_utf8_lossy(&rsdp.oem_id).trim().into();
    let revision = rsdp.revision;

    // Use XSDT (64-bit pointers) when available, else fall back to RSDT.
    let (sdt_phys, use_xsdt) = if revision >= 2 {
        let ext = unsafe { ptr::read_unaligned(rsdp_virt as *const RsdpExtended) };
        if ext.xsdt_address != 0 { (ext.xsdt_address, true) }
        else { (rsdp.rsdt_address as u64, false) }
    } else {
        (rsdp.rsdt_address as u64, false)
    };

    let sdt_virt = (sdt_phys + phys_offset) as *const u8;
    let sdt_header = unsafe { ptr::read_unaligned(sdt_virt as *const SdtHeader) };
    if !checksum_valid(sdt_virt, sdt_header.length as usize) { return None; }

    // Walk the pointer array following the SDT header.
    let hdr_sz = mem::size_of::<SdtHeader>();
    let ptr_sz = if use_xsdt { 8usize } else { 4 };
    let count = (sdt_header.length as usize - hdr_sz) / ptr_sz;

    let mut table_signatures = Vec::new();
    let mut madt: Option<Madt> = None;
    let mut fadt: Option<Fadt> = None;

    for i in 0..count {
        let entry_phys: u64 = if use_xsdt {
            unsafe { ptr::read_unaligned(sdt_virt.add(hdr_sz + i * 8) as *const u64) }
        } else {
            unsafe { ptr::read_unaligned(sdt_virt.add(hdr_sz + i * 4) as *const u32) as u64 }
        };

        let entry_virt = (entry_phys + phys_offset) as *const u8;
        let entry_hdr = unsafe { ptr::read_unaligned(entry_virt as *const SdtHeader) };
        let sig = entry_hdr.signature_str();
        table_signatures.push(sig.clone());

        match sig.as_str() {
            "APIC" => madt = Some(parse_madt(entry_virt)),
            "FACP" => fadt = Some(parse_fadt(entry_virt)),
            _ => {}
        }
    }

    Some(AcpiInfo { rsdp_revision: revision, oem_id, table_signatures, madt, fadt })
}

impl AcpiInfo {
    /// Produce a human-readable summary of all discovered ACPI information.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("ACPI revision: {}\n", self.rsdp_revision));
        s.push_str(&format!("OEM: {}\n", self.oem_id));
        s.push_str(&format!("Tables: {}\n", self.table_signatures.join(", ")));

        if let Some(ref m) = self.madt {
            s.push_str(&format!(
                "MADT: Local APIC @ {:#010x}, {} CPUs, {} I/O APICs\n",
                m.local_apic_address, m.local_apics.len(), m.io_apics.len(),
            ));
            for lap in &m.local_apics {
                let st = if lap.flags & 1 != 0 { "enabled" } else { "disabled" };
                s.push_str(&format!(
                    "  CPU proc_id={} apic_id={} [{}]\n",
                    lap.acpi_processor_id, lap.apic_id, st,
                ));
            }
            for ioa in &m.io_apics {
                s.push_str(&format!(
                    "  I/O APIC id={} addr={:#010x} GSI base={}\n",
                    ioa.io_apic_id, ioa.io_apic_address, ioa.global_system_interrupt_base,
                ));
            }
        } else {
            s.push_str("MADT: not found\n");
        }

        if let Some(ref f) = self.fadt {
            s.push_str(&format!(
                "FADT: PM1a={:#06x} PM1b={:#06x} SCI_INT={} SMI_CMD={:#06x}\n",
                f.pm1a_cnt_blk, f.pm1b_cnt_blk, f.sci_interrupt, f.smi_cmd,
            ));
            s.push_str(&format!("  Reset reg={:#010x} value={:#04x}\n", f.reset_reg_address, f.reset_value));
            s.push_str(&format!("  DSDT @ {:#010x}\n", f.dsdt_address));
        } else {
            s.push_str("FADT: not found\n");
        }
        s
    }
}
