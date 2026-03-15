/// Simplified ext2 filesystem implementation (read-only).
///
/// Supports superblock parsing, block group descriptors, inode reading
/// with direct/indirect block pointers, directory entry parsing, and
/// path-based file lookup.
///
/// Disk layout:
///   Bytes 0-1023:     Boot block (unused by ext2)
///   Bytes 1024-2047:  Superblock
///   Next block:       Block group descriptor table
///   Remaining:        Inode tables and data blocks per group descriptor

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Block device trait
// ---------------------------------------------------------------------------

/// Trait for reading raw bytes from a block device.
///
/// Implementors provide byte-level access; the ext2 driver handles
/// block-size alignment and multi-block reads internally.
pub trait BlockReader {
    /// Read `buf.len()` bytes starting at byte `offset`.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str>;
}

// ---------------------------------------------------------------------------
// On-disk structures
// ---------------------------------------------------------------------------

/// Ext2 superblock — maps the first 84 bytes of the 1024-byte on-disk structure.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub inodes_count: u32,
    pub blocks_count: u32,
    pub r_blocks_count: u32,
    pub free_blocks_count: u32,
    pub free_inodes_count: u32,
    /// First data block (1 for 1K block size, 0 for larger).
    pub first_data_block: u32,
    /// Block size = 1024 << log_block_size.
    pub log_block_size: u32,
    pub log_frag_size: u32,
    pub blocks_per_group: u32,
    pub frags_per_group: u32,
    pub inodes_per_group: u32,
    pub mtime: u32,
    pub wtime: u32,
    pub mnt_count: u16,
    pub max_mnt_count: u16,
    /// Must be `EXT2_MAGIC` (0xEF53).
    pub magic: u16,
    pub state: u16,
    pub errors: u16,
    pub minor_rev_level: u16,
    pub lastcheck: u32,
    pub checkinterval: u32,
    pub creator_os: u32,
    /// 0 = original format, 1 = dynamic inode sizes.
    pub rev_level: u32,
    pub def_resuid: u16,
    pub def_resgid: u16,
    /// First non-reserved inode (rev 1+).
    pub first_ino: u32,
    /// Inode size in bytes (128 for rev 0).
    pub inode_size: u16,
}

/// Ext2 magic number.
pub const EXT2_MAGIC: u16 = 0xEF53;

/// Root directory inode number (always 2 in ext2).
pub const ROOT_INODE: u32 = 2;

/// Block group descriptor (32 bytes on disk).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BlockGroupDesc {
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    /// First block of the inode table for this group.
    pub inode_table: u32,
    pub free_blocks_count: u16,
    pub free_inodes_count: u16,
    pub used_dirs_count: u16,
    pub pad: u16,
    pub reserved: [u8; 12],
}

/// On-disk inode (128 bytes for rev 0).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    /// File type and permissions (see `INODE_TYPE_MASK`).
    pub mode: u16,
    pub uid: u16,
    /// Lower 32 bits of file size.
    pub size: u32,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub gid: u16,
    pub links_count: u16,
    /// Number of 512-byte sectors allocated.
    pub blocks: u32,
    pub flags: u32,
    pub osd1: u32,
    /// 12 direct block pointers.
    pub direct: [u32; 12],
    /// Singly-indirect block pointer.
    pub indirect: u32,
    /// Doubly-indirect block pointer.
    pub double_indirect: u32,
    /// Triply-indirect block pointer.
    pub triple_indirect: u32,
    pub generation: u32,
    pub file_acl: u32,
    /// Upper 32 bits of file size (rev 1 regular files) or directory ACL.
    pub dir_acl: u32,
    pub faddr: u32,
    pub osd2: [u8; 12],
}

/// Inode type bitmask constants.
pub const INODE_TYPE_MASK: u16 = 0xF000;
pub const INODE_FILE: u16 = 0x8000;
pub const INODE_DIR: u16 = 0x4000;
pub const INODE_SYMLINK: u16 = 0xA000;

/// On-disk directory entry header (variable-length; name follows immediately).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntryRaw {
    pub inode: u32,
    /// Total record length including padding.
    pub rec_len: u16,
    pub name_len: u8,
    /// File type: 1 = regular, 2 = directory, 7 = symlink.
    pub file_type: u8,
}

/// Parsed directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub inode: u32,
    pub name: String,
    /// File type byte (1 = regular, 2 = directory, 7 = symlink).
    pub file_type: u8,
}

// ---------------------------------------------------------------------------
// Filesystem handle
// ---------------------------------------------------------------------------

/// Read-only ext2 filesystem driver.
///
/// Caches the superblock and block group descriptor table after mounting.
pub struct Ext2<'a, D: BlockReader> {
    dev: &'a D,
    /// Parsed superblock.
    pub sb: Superblock,
    /// Block size in bytes (1024, 2048, or 4096).
    pub block_size: usize,
    groups: Vec<BlockGroupDesc>,
}

impl<'a, D: BlockReader> Ext2<'a, D> {
    /// Mount an ext2 filesystem from the given block reader.
    ///
    /// Validates the superblock magic and loads all block group descriptors.
    pub fn mount(dev: &'a D) -> Result<Self, &'static str> {
        let sb = read_struct::<Superblock>(dev, 1024)?;
        if sb.magic != EXT2_MAGIC {
            return Err("ext2: bad magic number");
        }

        let block_size = 1024usize << sb.log_block_size;
        let num_groups = (sb.blocks_count as usize + sb.blocks_per_group as usize - 1)
            / sb.blocks_per_group as usize;

        // BGDT starts in the block immediately after the superblock.
        let bgdt_offset = if block_size == 1024 { 2048u64 } else { block_size as u64 };

        let mut groups = Vec::with_capacity(num_groups);
        for i in 0..num_groups {
            let off = bgdt_offset + (i * core::mem::size_of::<BlockGroupDesc>()) as u64;
            groups.push(read_struct::<BlockGroupDesc>(dev, off)?);
        }

        Ok(Self { dev, sb, block_size, groups })
    }

    /// Read a full block into `buf` (which must be >= `block_size` bytes).
    /// Block 0 is treated as a hole and yields zeroes.
    fn read_block(&self, block_num: u32, buf: &mut [u8]) -> Result<(), &'static str> {
        if block_num == 0 {
            for b in buf.iter_mut().take(self.block_size) { *b = 0; }
            return Ok(());
        }
        let offset = block_num as u64 * self.block_size as u64;
        self.dev.read_bytes(offset, &mut buf[..self.block_size])
    }

    /// Read the on-disk inode for the given 1-based inode number.
    pub fn read_inode(&self, ino: u32) -> Result<Inode, &'static str> {
        if ino == 0 {
            return Err("ext2: inode 0 is invalid");
        }
        let idx = (ino - 1) as usize;
        let group = idx / self.sb.inodes_per_group as usize;
        let local = idx % self.sb.inodes_per_group as usize;
        if group >= self.groups.len() {
            return Err("ext2: inode group out of range");
        }
        let inode_sz = if self.sb.rev_level >= 1 { self.sb.inode_size as usize } else { 128 };
        let off = self.groups[group].inode_table as u64 * self.block_size as u64
            + local as u64 * inode_sz as u64;
        read_struct::<Inode>(self.dev, off)
    }

    /// Collect all data block numbers for an inode, resolving indirect pointers.
    fn collect_blocks(&self, inode: &Inode) -> Result<Vec<u32>, &'static str> {
        let total = (inode.size as usize + self.block_size - 1) / self.block_size;
        let mut out = Vec::with_capacity(total);
        let ptrs_per = self.block_size / 4;

        // 12 direct block pointers.
        for i in 0..12 {
            if out.len() >= total { return Ok(out); }
            out.push(inode.direct[i]);
        }

        // Singly-indirect.
        if out.len() < total && inode.indirect != 0 {
            self.append_indirect(inode.indirect, &mut out, total)?;
        }

        // Doubly-indirect.
        if out.len() < total && inode.double_indirect != 0 {
            let l1 = self.read_block_ptrs(inode.double_indirect)?;
            for &p in l1.iter().take(ptrs_per) {
                if out.len() >= total { break; }
                if p != 0 { self.append_indirect(p, &mut out, total)?; }
            }
        }

        // Triply-indirect.
        if out.len() < total && inode.triple_indirect != 0 {
            let l1 = self.read_block_ptrs(inode.triple_indirect)?;
            for &p1 in l1.iter().take(ptrs_per) {
                if out.len() >= total { break; }
                if p1 == 0 { continue; }
                let l2 = self.read_block_ptrs(p1)?;
                for &p2 in l2.iter().take(ptrs_per) {
                    if out.len() >= total { break; }
                    if p2 != 0 { self.append_indirect(p2, &mut out, total)?; }
                }
            }
        }

        Ok(out)
    }

    /// Read one block of `u32` pointers (for indirect block resolution).
    fn read_block_ptrs(&self, block_num: u32) -> Result<Vec<u32>, &'static str> {
        let mut buf = vec![0u8; self.block_size];
        self.read_block(block_num, &mut buf)?;
        let n = self.block_size / 4;
        let mut ptrs = Vec::with_capacity(n);
        for i in 0..n {
            let o = i * 4;
            ptrs.push(u32::from_le_bytes([buf[o], buf[o+1], buf[o+2], buf[o+3]]));
        }
        Ok(ptrs)
    }

    /// Append pointers from a singly-indirect block, up to `limit` total.
    fn append_indirect(&self, blk: u32, out: &mut Vec<u32>, limit: usize) -> Result<(), &'static str> {
        let ptrs = self.read_block_ptrs(blk)?;
        for &p in &ptrs {
            if out.len() >= limit { break; }
            out.push(p);
        }
        Ok(())
    }

    /// Read the full contents of a file by inode number.
    ///
    /// Returns exactly `inode.size` bytes.
    pub fn read_file_by_inode(&self, ino: u32) -> Result<Vec<u8>, &'static str> {
        let inode = self.read_inode(ino)?;
        let size = inode.size as usize;
        if size == 0 { return Ok(Vec::new()); }

        let blks = self.collect_blocks(&inode)?;
        let mut data = Vec::with_capacity(size);
        let mut tmp = vec![0u8; self.block_size];
        let mut rem = size;

        for &b in &blks {
            self.read_block(b, &mut tmp)?;
            let n = rem.min(self.block_size);
            data.extend_from_slice(&tmp[..n]);
            rem -= n;
            if rem == 0 { break; }
        }
        Ok(data)
    }

    /// List all entries in a directory (by inode number).
    ///
    /// Returns `Err` if the inode is not a directory.
    pub fn list_directory(&self, dir_ino: u32) -> Result<Vec<DirEntry>, &'static str> {
        let inode = self.read_inode(dir_ino)?;
        if inode.mode & INODE_TYPE_MASK != INODE_DIR {
            return Err("ext2: not a directory");
        }
        let raw = self.read_file_by_inode(dir_ino)?;
        parse_dir_entries(&raw)
    }

    /// Resolve an absolute UNIX path (e.g. "/usr/bin/hello") to an inode number.
    ///
    /// Walks from the root inode through each path component.
    pub fn lookup_path(&self, path: &str) -> Result<u32, &'static str> {
        let mut cur = ROOT_INODE;
        for part in path.split('/') {
            if part.is_empty() { continue; }
            let entries = self.list_directory(cur)?;
            match entries.iter().find(|e| e.name == part) {
                Some(e) => cur = e.inode,
                None => return Err("ext2: path component not found"),
            }
        }
        Ok(cur)
    }

    /// Convenience: read a file by absolute path.
    pub fn read_file_by_path(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        let ino = self.lookup_path(path)?;
        self.read_file_by_inode(ino)
    }

    /// Return the file size for an inode (handles 64-bit sizes for rev 1).
    pub fn file_size(&self, ino: u32) -> Result<u64, &'static str> {
        let inode = self.read_inode(ino)?;
        if inode.mode & INODE_TYPE_MASK == INODE_FILE && self.sb.rev_level >= 1 {
            Ok((inode.dir_acl as u64) << 32 | inode.size as u64)
        } else {
            Ok(inode.size as u64)
        }
    }

    /// Return the number of block groups.
    pub fn num_groups(&self) -> usize { self.groups.len() }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a `#[repr(C, packed)]` struct from the device at byte `offset`.
fn read_struct<T: Copy>(dev: &dyn BlockReader, offset: u64) -> Result<T, &'static str> {
    let size = core::mem::size_of::<T>();
    let mut buf = vec![0u8; size];
    dev.read_bytes(offset, &mut buf)?;
    // SAFETY: T is repr(C, packed) and buf holds exactly `size` bytes.
    Ok(unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const T) })
}

/// Parse raw directory data into a list of [`DirEntry`] values.
///
/// Walks the variable-length linked list of ext2 directory records,
/// skipping entries where inode == 0 (deleted or padding).
fn parse_dir_entries(data: &[u8]) -> Result<Vec<DirEntry>, &'static str> {
    let mut entries = Vec::new();
    let mut pos = 0usize;
    let len = data.len();

    while pos + 8 <= len {
        let inode = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        let rec_len = u16::from_le_bytes([data[pos+4], data[pos+5]]) as usize;
        let name_len = data[pos+6] as usize;
        let file_type = data[pos+7];

        if rec_len == 0 { break; } // guard against corrupt data

        if inode != 0 && pos + 8 + name_len <= len {
            let name = core::str::from_utf8(&data[pos+8..pos+8+name_len])
                .unwrap_or("<invalid utf8>")
                .into();
            entries.push(DirEntry { inode, name, file_type });
        }
        pos += rec_len;
    }
    Ok(entries)
}
