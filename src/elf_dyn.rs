/// ELF dynamic linker foundation for MerlionOS.
///
/// Parses the `.dynamic` section to extract shared library dependencies,
/// string/symbol tables, and relocation entries. Applies x86_64 relocations
/// (R_X86_64_64, R_X86_64_PC32, R_X86_64_RELATIVE, GLOB_DAT, JUMP_SLOT).

use alloc::string::String;
use alloc::vec::Vec;

// Dynamic section tag constants (Elf64_Dyn.d_tag)
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_PLTRELSZ: u64 = 2;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_STRSZ: u64 = 10;
const DT_SYMENT: u64 = 11;
const DT_JMPREL: u64 = 23;

/// S + A -- absolute 64-bit relocation.
pub const R_X86_64_64: u32 = 1;
/// S + A - P -- PC-relative 32-bit relocation.
pub const R_X86_64_PC32: u32 = 2;
/// S -- GOT entry for a global data symbol.
pub const R_X86_64_GLOB_DAT: u32 = 6;
/// S -- PLT jump slot (lazy or eager binding).
pub const R_X86_64_JUMP_SLOT: u32 = 7;
/// B + A -- base-relative relocation (no symbol).
pub const R_X86_64_RELATIVE: u32 = 8;

/// A single entry in the `.dynamic` section.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Elf64Dyn {
    d_tag: u64,
    d_val: u64,
}

/// A single Rela relocation entry (with explicit addend).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Rela {
    /// Offset within the object where the relocation applies.
    pub r_offset: u64,
    /// Packed: upper 32 bits = symbol index, lower 32 bits = reloc type.
    pub r_info: u64,
    /// Signed addend used to compute the relocated value.
    pub r_addend: i64,
}

impl Elf64Rela {
    /// Extract the relocation type from `r_info`.
    pub fn reloc_type(&self) -> u32 {
        (self.r_info & 0xFFFF_FFFF) as u32
    }
    /// Extract the symbol table index from `r_info`.
    pub fn sym_index(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
}

/// A single entry in the `.dynsym` symbol table.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Sym {
    /// Index into the string table for the symbol name.
    pub st_name: u32,
    /// Symbol type (low 4 bits) and binding (high 4 bits).
    pub st_info: u8,
    /// Symbol visibility.
    pub st_other: u8,
    /// Section header index the symbol is defined in.
    pub st_shndx: u16,
    /// Symbol value (address).
    pub st_value: u64,
    /// Symbol size in bytes.
    pub st_size: u64,
}

impl Elf64Sym {
    /// Symbol binding: 0=LOCAL, 1=GLOBAL, 2=WEAK.
    pub fn binding(&self) -> u8 { self.st_info >> 4 }
    /// Symbol type: 0=NOTYPE, 1=OBJECT, 2=FUNC, 3=SECTION, 4=FILE.
    pub fn sym_type(&self) -> u8 { self.st_info & 0xF }
    /// True if this symbol is undefined (needs resolution from another object).
    pub fn is_undefined(&self) -> bool { self.st_shndx == 0 }
}

/// Aggregated information extracted from the `.dynamic` section.
#[derive(Debug)]
pub struct DynInfo {
    /// Names of shared libraries listed as DT_NEEDED.
    pub needed_libs: Vec<String>,
    /// Virtual address of the string table.
    pub strtab_addr: u64,
    /// Byte size of the string table.
    pub strtab_size: u64,
    /// Virtual address of the symbol table.
    pub symtab_addr: u64,
    /// Byte size of a single symbol entry.
    pub syment_size: u64,
    /// Rela relocation entries from the DT_RELA table.
    pub rela_entries: Vec<Elf64Rela>,
    /// PLT relocation entries from the DT_JMPREL table.
    pub plt_rela_entries: Vec<Elf64Rela>,
}

/// Parse the ELF `.dynamic` section from raw ELF image bytes.
///
/// `data` is the full ELF image. `base` is the load base address -- virtual
/// addresses in the dynamic section are offset by this value for PIE binaries.
/// Returns `DynInfo` with needed libraries, table locations, and relocations.
pub fn parse_dynamic_section(data: &[u8], base: u64) -> Result<DynInfo, &'static str> {
    if data.len() < 64 {
        return Err("data too short for ELF header");
    }

    let e_phoff = u64_at(data, 32) as usize;
    let e_phentsize = u16_at(data, 54) as usize;
    let e_phnum = u16_at(data, 56) as usize;

    // Locate PT_DYNAMIC program header (p_type == 2).
    let mut dyn_offset: usize = 0;
    let mut dyn_size: usize = 0;
    let mut found = false;
    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + e_phentsize > data.len() { break; }
        if u32_at(data, off) == 2 {
            dyn_offset = u64_at(data, off + 8) as usize;
            dyn_size = u64_at(data, off + 32) as usize;
            found = true;
            break;
        }
    }
    if !found {
        return Err("no PT_DYNAMIC segment found");
    }
    if dyn_offset + dyn_size > data.len() {
        return Err("PT_DYNAMIC extends beyond file");
    }

    // Walk dynamic entries.
    let esz = core::mem::size_of::<Elf64Dyn>();
    let num = dyn_size / esz;

    let mut strtab_addr: u64 = 0;
    let mut strtab_size: u64 = 0;
    let mut symtab_addr: u64 = 0;
    let mut syment_size: u64 = 24;
    let mut rela_addr: u64 = 0;
    let mut rela_size: u64 = 0;
    let mut rela_ent: u64 = 24;
    let mut jmprel_addr: u64 = 0;
    let mut jmprel_size: u64 = 0;
    let mut needed_offsets: Vec<u64> = Vec::new();

    for i in 0..num {
        let off = dyn_offset + i * esz;
        if off + esz > data.len() { break; }
        let d_tag = u64_at(data, off);
        let d_val = u64_at(data, off + 8);
        match d_tag {
            DT_NULL     => break,
            DT_NEEDED   => needed_offsets.push(d_val),
            DT_STRTAB   => strtab_addr = d_val,
            DT_STRSZ    => strtab_size = d_val,
            DT_SYMTAB   => symtab_addr = d_val,
            DT_SYMENT   => syment_size = d_val,
            DT_RELA     => rela_addr = d_val,
            DT_RELASZ   => rela_size = d_val,
            DT_RELAENT  => rela_ent = d_val,
            DT_JMPREL   => jmprel_addr = d_val,
            DT_PLTRELSZ => jmprel_size = d_val,
            _ => {}
        }
    }

    // Convert strtab vaddr to file offset, resolve DT_NEEDED names.
    let strtab_foff = strtab_addr.wrapping_sub(base) as usize;
    let mut needed_libs = Vec::new();
    for &str_off in &needed_offsets {
        let start = strtab_foff + str_off as usize;
        if start < data.len() {
            needed_libs.push(read_cstr(data, start));
        }
    }

    let rela_entries = read_rela_table(data, rela_addr, rela_size, rela_ent, base);
    let plt_rela_entries = read_rela_table(data, jmprel_addr, jmprel_size, rela_ent, base);

    Ok(DynInfo {
        needed_libs, strtab_addr, strtab_size,
        symtab_addr, syment_size, rela_entries, plt_rela_entries,
    })
}

/// Read a Rela relocation table from the ELF image.
fn read_rela_table(
    data: &[u8], addr: u64, total: u64, entsz: u64, base: u64,
) -> Vec<Elf64Rela> {
    let mut out = Vec::new();
    if addr == 0 || total == 0 || entsz == 0 { return out; }
    let foff = addr.wrapping_sub(base) as usize;
    let count = (total / entsz) as usize;
    let esz = entsz as usize;
    for i in 0..count {
        let off = foff + i * esz;
        if off + 24 > data.len() { break; }
        out.push(Elf64Rela {
            r_offset: u64_at(data, off),
            r_info:   u64_at(data, off + 8),
            r_addend: u64_at(data, off + 16) as i64,
        });
    }
    out
}

/// Resolve a symbol by name from the dynamic symbol and string tables.
///
/// Performs a linear scan of `symtab`, comparing each entry's name (looked
/// up in `strtab`) against `name`. Returns the matching `Elf64Sym` entry.
pub fn resolve_symbol<'a>(
    name: &str, symtab: &'a [Elf64Sym], strtab: &[u8],
) -> Option<&'a Elf64Sym> {
    for sym in symtab.iter() {
        let start = sym.st_name as usize;
        if start >= strtab.len() { continue; }
        if read_cstr(strtab, start) == name {
            return Some(sym);
        }
    }
    None
}

/// Apply an array of Rela relocations at `base_addr`.
///
/// `sym_resolver` is called with the symbol index for relocations that need
/// a symbol value; it is not called for `R_X86_64_RELATIVE`.
///
/// Returns the number of relocations successfully applied.
///
/// # Safety
///
/// The caller must ensure `base_addr` plus relocation offsets point to
/// valid, writable memory.
pub unsafe fn apply_relocations(
    base_addr: u64,
    rela_entries: &[Elf64Rela],
    sym_resolver: &dyn Fn(u32) -> Option<u64>,
) -> Result<usize, &'static str> {
    let mut applied = 0usize;
    for rela in rela_entries {
        let target = (base_addr + rela.r_offset) as *mut u64;
        let rtype = rela.reloc_type();
        let sym_idx = rela.sym_index();
        match rtype {
            R_X86_64_RELATIVE => {
                let val = (base_addr as i64 + rela.r_addend) as u64;
                core::ptr::write_unaligned(target, val);
            }
            R_X86_64_64 => {
                let sv = sym_resolver(sym_idx)
                    .ok_or("unresolved symbol for R_X86_64_64")?;
                let val = (sv as i64 + rela.r_addend) as u64;
                core::ptr::write_unaligned(target, val);
            }
            R_X86_64_PC32 => {
                let sv = sym_resolver(sym_idx)
                    .ok_or("unresolved symbol for R_X86_64_PC32")?;
                let pc = base_addr + rela.r_offset;
                let val = (sv as i64 + rela.r_addend - pc as i64) as u32;
                core::ptr::write_unaligned(target as *mut u32, val);
            }
            R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                let sv = sym_resolver(sym_idx)
                    .ok_or("unresolved symbol for GOT/PLT relocation")?;
                core::ptr::write_unaligned(target, sv);
            }
            _ => continue, // skip unsupported types
        }
        applied += 1;
    }
    Ok(applied)
}

// -- Byte-reading helpers ---------------------------------------------------

/// Read a little-endian u64 from `data` at byte offset `off`.
fn u64_at(data: &[u8], off: usize) -> u64 {
    let b = &data[off..off + 8];
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Read a little-endian u32 from `data` at byte offset `off`.
fn u32_at(data: &[u8], off: usize) -> u32 {
    let b = &data[off..off + 4];
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Read a little-endian u16 from `data` at byte offset `off`.
fn u16_at(data: &[u8], off: usize) -> u16 {
    let b = &data[off..off + 2];
    u16::from_le_bytes([b[0], b[1]])
}

/// Read a NUL-terminated C string from `data` starting at byte offset `off`.
fn read_cstr(data: &[u8], off: usize) -> String {
    let mut end = off;
    while end < data.len() && data[end] != 0 { end += 1; }
    String::from_utf8_lossy(&data[off..end]).into_owned()
}
