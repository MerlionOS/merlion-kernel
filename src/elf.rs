/// Minimal ELF-64 binary parser.
/// Parses ELF headers and program headers for loading user binaries.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// ELF magic number.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF-64 file header (first 64 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Header {
    pub magic: [u8; 4],
    pub class: u8,        // 1=32-bit, 2=64-bit
    pub endian: u8,       // 1=little, 2=big
    pub version: u8,
    pub os_abi: u8,
    pub _pad: [u8; 8],
    pub elf_type: u16,    // 1=relocatable, 2=executable, 3=shared
    pub machine: u16,     // 0x3E = x86_64
    pub version2: u32,
    pub entry: u64,       // entry point virtual address
    pub ph_offset: u64,   // program header table offset
    pub sh_offset: u64,   // section header table offset
    pub flags: u32,
    pub header_size: u16,
    pub ph_entry_size: u16,
    pub ph_count: u16,
    pub sh_entry_size: u16,
    pub sh_count: u16,
    pub sh_strndx: u16,
}

/// ELF-64 program header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64ProgramHeader {
    pub seg_type: u32,    // 1=LOAD
    pub flags: u32,       // 1=X, 2=W, 4=R
    pub offset: u64,      // offset in file
    pub vaddr: u64,       // virtual address
    pub paddr: u64,       // physical address
    pub file_size: u64,   // size in file
    pub mem_size: u64,    // size in memory
    pub align: u64,
}

/// Parsed ELF info.
pub struct ElfInfo {
    pub entry_point: u64,
    pub machine: &'static str,
    pub elf_type: &'static str,
    pub segments: Vec<SegmentInfo>,
    pub valid: bool,
}

pub struct SegmentInfo {
    pub seg_type: &'static str,
    pub vaddr: u64,
    pub mem_size: u64,
    pub flags: String,
}

/// Parse an ELF binary from raw bytes.
pub fn parse(data: &[u8]) -> Result<ElfInfo, &'static str> {
    if data.len() < 64 {
        return Err("too small for ELF header");
    }
    if data[0..4] != ELF_MAGIC {
        return Err("not an ELF file");
    }
    if data[4] != 2 {
        return Err("not 64-bit ELF");
    }

    let header = unsafe { &*(data.as_ptr() as *const Elf64Header) };

    let machine = match header.machine {
        0x3E => "x86_64",
        0xB7 => "aarch64",
        0x03 => "x86",
        _ => "unknown",
    };

    let elf_type = match header.elf_type {
        1 => "relocatable",
        2 => "executable",
        3 => "shared",
        4 => "core",
        _ => "unknown",
    };

    let mut segments = Vec::new();
    let ph_offset = header.ph_offset as usize;
    let ph_size = header.ph_entry_size as usize;
    let ph_count = header.ph_count as usize;

    for i in 0..ph_count {
        let off = ph_offset + i * ph_size;
        if off + ph_size > data.len() { break; }

        let ph = unsafe { &*(data[off..].as_ptr() as *const Elf64ProgramHeader) };

        let seg_type = match ph.seg_type {
            0 => "NULL",
            1 => "LOAD",
            2 => "DYNAMIC",
            3 => "INTERP",
            4 => "NOTE",
            6 => "PHDR",
            _ => "OTHER",
        };

        let mut flags = String::new();
        if ph.flags & 4 != 0 { flags.push('R'); }
        if ph.flags & 2 != 0 { flags.push('W'); }
        if ph.flags & 1 != 0 { flags.push('X'); }

        segments.push(SegmentInfo {
            seg_type,
            vaddr: ph.vaddr,
            mem_size: ph.mem_size,
            flags,
        });
    }

    Ok(ElfInfo {
        entry_point: header.entry,
        machine,
        elf_type,
        segments,
        valid: true,
    })
}

/// Format ELF info for display.
pub fn format_info(info: &ElfInfo) -> String {
    let mut out = format!(
        "ELF {} {} — entry: {:#x}\n",
        info.machine, info.elf_type, info.entry_point
    );
    if !info.segments.is_empty() {
        out.push_str("Segments:\n");
        for seg in &info.segments {
            out.push_str(&format!(
                "  {:8} vaddr={:#010x} size={:#x} [{}]\n",
                seg.seg_type, seg.vaddr, seg.mem_size, seg.flags
            ));
        }
    }
    out
}
