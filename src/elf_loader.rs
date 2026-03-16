/// ELF Loader — loads and executes ELF binaries from memory or disk.
/// Parses ELF-64 program headers, maps LOAD segments into a user
/// page table, and enters ring 3 at the ELF entry point.

use crate::{elf, memory, serial_println, klog_println, virtio_blk};
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;
use alloc::vec::Vec;

/// Load and execute an ELF binary from a byte slice.
pub fn load_and_exec(name: &str, data: &[u8]) -> Result<(), &'static str> {
    // Parse ELF
    let info = elf::parse(data)?;
    serial_println!("[elf-loader] {} — {} {}, entry {:#x}",
        name, info.machine, info.elf_type, info.entry_point);

    if info.machine != "x86_64" {
        return Err("unsupported architecture (need x86_64)");
    }

    // Create user page table
    let (pml4_frame, mut mapper) =
        memory::create_user_page_table().ok_or("failed to create page table")?;

    // Map each LOAD segment
    let mut mapped_pages = 0u64;
    for seg in &info.segments {
        if seg.seg_type != "LOAD" || seg.mem_size == 0 {
            continue;
        }

        let num_pages = (seg.mem_size + 4095) / 4096;
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if seg.flags.contains('W') {
            flags |= PageTableFlags::WRITABLE;
        }

        for p in 0..num_pages {
            let vaddr = seg.vaddr + p * 4096;
            let page = Page::containing_address(VirtAddr::new(vaddr));

            let frame = memory::map_page(&mut mapper, page, flags)
                .ok_or("out of memory mapping ELF segment")?;

            // Copy data from ELF file to the mapped page
            let dest = memory::phys_to_virt(frame.start_address());
            unsafe {
                // Zero the page first
                core::ptr::write_bytes(dest.as_mut_ptr::<u8>(), 0, 4096);
            }

            // Find corresponding file data for this page
            // We need to look at the raw ELF program headers
            let file_offset = find_file_offset(data, seg.vaddr, p * 4096);
            if let Some(offset) = file_offset {
                let remaining_in_file = data.len().saturating_sub(offset);
                let to_copy = 4096.min(remaining_in_file);
                if to_copy > 0 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data[offset..].as_ptr(),
                            dest.as_mut_ptr::<u8>(),
                            to_copy,
                        );
                    }
                }
            }

            mapped_pages += 1;
        }
    }

    serial_println!("[elf-loader] mapped {} pages", mapped_pages);

    // Map user stack (2 pages at 0x800000)
    let stack_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    for i in 0..2u64 {
        let stack_page = Page::containing_address(VirtAddr::new(0x800000 - (i + 1) * 4096));
        memory::map_page(&mut mapper, stack_page, stack_flags)
            .ok_or("out of memory for user stack")?;
    }

    // Enter ring 3
    serial_println!("[elf-loader] entering ring 3 at {:#x}", info.entry_point);
    klog_println!("[elf-loader] exec '{}' at {:#x}", name, info.entry_point);

    let pml4_phys = pml4_frame.start_address();
    enter_user(pml4_phys.as_u64(), info.entry_point, 0x800000);

    serial_println!("[elf-loader] '{}' returned to kernel", name);
    Ok(())
}

/// Load an ELF from the virtio disk (starting at given sector, size in bytes).
pub fn load_from_disk(start_sector: u64, size: usize) -> Result<Vec<u8>, &'static str> {
    if !virtio_blk::is_detected() {
        return Err("no virtio disk");
    }

    let sectors_needed = (size + 511) / 512;
    let mut data = Vec::with_capacity(sectors_needed * 512);

    for i in 0..sectors_needed as u64 {
        let mut buf = [0u8; 512];
        virtio_blk::read_sector(start_sector + i, &mut buf)?;
        data.extend_from_slice(&buf);
    }

    data.truncate(size);
    Ok(data)
}

/// Build a minimal ELF-64 binary from raw machine code.
/// Places code at virtual address 0x400000 with a proper ELF header.
pub fn build_elf(code: &[u8]) -> Vec<u8> {
    let entry: u64 = 0x400000;
    let code_offset: u64 = 0x78; // after ELF header (64) + 1 program header (56) = 120 = 0x78

    let mut elf = Vec::new();

    // ELF header (64 bytes)
    elf.extend_from_slice(&[0x7F, b'E', b'L', b'F']); // magic
    elf.push(2);    // class: 64-bit
    elf.push(1);    // endian: little
    elf.push(1);    // version
    elf.push(0);    // OS/ABI
    elf.extend_from_slice(&[0; 8]); // padding
    elf.extend_from_slice(&2u16.to_le_bytes()); // type: executable
    elf.extend_from_slice(&0x3Eu16.to_le_bytes()); // machine: x86_64
    elf.extend_from_slice(&1u32.to_le_bytes()); // version
    elf.extend_from_slice(&entry.to_le_bytes()); // entry point
    elf.extend_from_slice(&64u64.to_le_bytes()); // ph_offset (right after header)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_offset (none)
    elf.extend_from_slice(&0u32.to_le_bytes()); // flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // header size
    elf.extend_from_slice(&56u16.to_le_bytes()); // ph entry size
    elf.extend_from_slice(&1u16.to_le_bytes()); // ph count
    elf.extend_from_slice(&0u16.to_le_bytes()); // sh entry size
    elf.extend_from_slice(&0u16.to_le_bytes()); // sh count
    elf.extend_from_slice(&0u16.to_le_bytes()); // sh_strndx

    // Program header (56 bytes) — PT_LOAD
    elf.extend_from_slice(&1u32.to_le_bytes()); // type: LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // flags: R+X
    elf.extend_from_slice(&code_offset.to_le_bytes()); // offset in file
    elf.extend_from_slice(&entry.to_le_bytes()); // vaddr
    elf.extend_from_slice(&entry.to_le_bytes()); // paddr
    elf.extend_from_slice(&(code.len() as u64).to_le_bytes()); // file_size
    elf.extend_from_slice(&(code.len() as u64).to_le_bytes()); // mem_size
    elf.extend_from_slice(&0x1000u64.to_le_bytes()); // align

    // Code
    elf.extend_from_slice(code);

    elf
}

/// Find file offset for a virtual address within a LOAD segment.
fn find_file_offset(elf_data: &[u8], seg_vaddr: u64, page_offset: u64) -> Option<usize> {
    if elf_data.len() < 64 { return None; }
    let header = unsafe { &*(elf_data.as_ptr() as *const elf::Elf64Header) };
    let ph_offset = header.ph_offset as usize;
    let ph_size = header.ph_entry_size as usize;
    let ph_count = header.ph_count as usize;

    for i in 0..ph_count {
        let off = ph_offset + i * ph_size;
        if off + ph_size > elf_data.len() { break; }
        let ph = unsafe { &*(elf_data[off..].as_ptr() as *const elf::Elf64ProgramHeader) };
        if ph.seg_type == 1 && ph.vaddr == seg_vaddr {
            let file_off = ph.offset as usize + page_offset as usize;
            if file_off < elf_data.len() {
                return Some(file_off);
            }
        }
    }
    None
}

fn enter_user(pml4_phys: u64, code_addr: u64, stack_top: u64) {
    let user_data_seg: u64 = (4 << 3) | 3;
    let user_code_seg: u64 = (5 << 3) | 3;

    unsafe {
        core::arch::asm!(
            "mov rax, cr3",
            "push rax",
            "mov rax, {pml4}",
            "mov cr3, rax",
            "push {user_ds}",
            "push {user_sp}",
            "pushfq",
            "pop rax",
            "or rax, 0x200",
            "push rax",
            "push {user_cs}",
            "push {user_ip}",
            "iretq",
            pml4 = in(reg) pml4_phys,
            user_ds = in(reg) user_data_seg,
            user_sp = in(reg) stack_top,
            user_cs = in(reg) user_code_seg,
            user_ip = in(reg) code_addr,
            out("rax") _,
        );
    }
}
