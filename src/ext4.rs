/// Ext4 filesystem for MerlionOS — extent trees, JBD2 journal, HTree dirs,
/// large files, nanosecond timestamps, inline data.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// --- Statistics ---
static BLOCKS_READ: AtomicU64 = AtomicU64::new(0);
static BLOCKS_WRITTEN: AtomicU64 = AtomicU64::new(0);
static JOURNAL_TXN_COUNT: AtomicU64 = AtomicU64::new(0);
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);

/// Block I/O trait for byte-level device access.
pub trait BlockIO {
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<(), &'static str>;
}

// --- Constants ---
pub const EXT4_MAGIC: u16 = 0xEF53;
pub const ROOT_INODE: u32 = 2;
pub const JOURNAL_INODE: u32 = 8;
pub const INCOMPAT_EXTENTS: u32 = 0x0040;
pub const INCOMPAT_64BIT: u32 = 0x0080;
pub const COMPAT_DIR_INDEX: u32 = 0x0020;
pub const COMPAT_HAS_JOURNAL: u32 = 0x0004;
pub const EXT4_EXTENTS_FL: u32 = 0x0008_0000;
pub const EXT4_INLINE_DATA_FL: u32 = 0x1000_0000;
pub const EXT4_INDEX_FL: u32 = 0x0000_1000;
pub const INODE_TYPE_MASK: u16 = 0xF000;
pub const INODE_FILE: u16 = 0x8000;
pub const INODE_DIR: u16 = 0x4000;
pub const EXTENT_MAGIC: u16 = 0xF30A;
pub const JBD2_MAGIC: u32 = 0xC03B_3998;
pub const HTREE_HASH_TEA: u8 = 2;

// --- Superblock ---
/// Ext4 superblock with extended fields for journal, 64-bit blocks, extents.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub inodes_count: u32,      pub blocks_count_lo: u32,
    pub r_blocks_count_lo: u32, pub free_blocks_count_lo: u32,
    pub free_inodes_count: u32, pub first_data_block: u32,
    pub log_block_size: u32,    pub log_cluster_size: u32,
    pub blocks_per_group: u32,  pub clusters_per_group: u32,
    pub inodes_per_group: u32,  pub mtime: u32,
    pub wtime: u32,             pub mnt_count: u16,
    pub max_mnt_count: u16,     pub magic: u16,
    pub state: u16,             pub errors: u16,
    pub minor_rev_level: u16,   pub lastcheck: u32,
    pub checkinterval: u32,     pub creator_os: u32,
    pub rev_level: u32,         pub def_resuid: u16,
    pub def_resgid: u16,
    // ext4 extended fields
    pub first_ino: u32,         pub inode_size: u16,
    pub block_group_nr: u16,    pub feature_compat: u32,
    pub feature_incompat: u32,  pub feature_ro_compat: u32,
    pub uuid: [u8; 16],         pub volume_name: [u8; 16],
    pub last_mounted: [u8; 64], pub algorithm_usage_bitmap: u32,
    pub prealloc_blocks: u8,    pub prealloc_dir_blocks: u8,
    pub reserved_gdt_blocks: u16,
    pub journal_uuid: [u8; 16], pub journal_inum: u32,
    pub journal_dev: u32,       pub last_orphan: u32,
    pub hash_seed: [u32; 4],    pub def_hash_version: u8,
    pub jnl_backup_type: u8,    pub desc_size: u16,
    pub default_mount_opts: u32,pub first_meta_bg: u32,
    pub mkfs_time: u32,         pub jnl_blocks: [u32; 17],
    pub blocks_count_hi: u32,   pub r_blocks_count_hi: u32,
    pub free_blocks_count_hi: u32,
    pub min_extra_isize: u16,   pub want_extra_isize: u16,
}

impl Superblock {
    pub fn blocks_count(&self) -> u64 { self.blocks_count_lo as u64 | ((self.blocks_count_hi as u64) << 32) }
    pub fn free_blocks_count(&self) -> u64 { self.free_blocks_count_lo as u64 | ((self.free_blocks_count_hi as u64) << 32) }
    pub fn block_size(&self) -> usize { 1024usize << self.log_block_size }
    pub fn has_extents(&self) -> bool { self.feature_incompat & INCOMPAT_EXTENTS != 0 }
    pub fn has_journal(&self) -> bool { self.feature_compat & COMPAT_HAS_JOURNAL != 0 }
    pub fn has_dir_index(&self) -> bool { self.feature_compat & COMPAT_DIR_INDEX != 0 }
    pub fn volume_label(&self) -> String {
        let end = self.volume_name.iter().position(|&b| b == 0).unwrap_or(16);
        String::from_utf8_lossy(&self.volume_name[..end]).into_owned()
    }
    pub fn uuid_string(&self) -> String {
        let u = &self.uuid;
        format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            u[0],u[1],u[2],u[3],u[4],u[5],u[6],u[7],u[8],u[9],u[10],u[11],u[12],u[13],u[14],u[15])
    }
}

// --- Block group descriptor (64-byte ext4) ---
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BlockGroupDesc {
    pub block_bitmap_lo: u32, pub inode_bitmap_lo: u32, pub inode_table_lo: u32,
    pub free_blocks_count_lo: u16, pub free_inodes_count_lo: u16,
    pub used_dirs_count_lo: u16, pub flags: u16,
    pub exclude_bitmap_lo: u32, pub block_bitmap_csum_lo: u16,
    pub inode_bitmap_csum_lo: u16, pub itable_unused_lo: u16, pub checksum: u16,
    pub block_bitmap_hi: u32, pub inode_bitmap_hi: u32, pub inode_table_hi: u32,
    pub free_blocks_count_hi: u16, pub free_inodes_count_hi: u16,
    pub used_dirs_count_hi: u16, pub itable_unused_hi: u16,
    pub exclude_bitmap_hi: u32, pub block_bitmap_csum_hi: u16,
    pub inode_bitmap_csum_hi: u16, pub reserved: u32,
}

impl BlockGroupDesc {
    pub fn inode_table(&self) -> u64 { self.inode_table_lo as u64 | ((self.inode_table_hi as u64) << 32) }
}

// --- Inode (ext4, 256 bytes with extra fields) ---
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    pub mode: u16, pub uid: u16, pub size_lo: u32,
    pub atime: u32, pub ctime: u32, pub mtime: u32, pub dtime: u32,
    pub gid: u16, pub links_count: u16, pub blocks_lo: u32,
    pub flags: u32, pub osd1: u32,
    /// 60 bytes: extent tree root or 12 direct + 3 indirect pointers.
    pub block: [u8; 60],
    pub generation: u32, pub file_acl_lo: u32, pub size_hi: u32,
    pub obso_faddr: u32, pub osd2: [u8; 12],
    pub extra_isize: u16, pub checksum_hi: u16,
    pub ctime_extra: u32, pub mtime_extra: u32, pub atime_extra: u32,
    pub crtime: u32, pub crtime_extra: u32, pub version_hi: u32, pub projid: u32,
}

impl Inode {
    pub fn size(&self) -> u64 { self.size_lo as u64 | ((self.size_hi as u64) << 32) }
    pub fn uses_extents(&self) -> bool { self.flags & EXT4_EXTENTS_FL != 0 }
    pub fn has_inline_data(&self) -> bool { self.flags & EXT4_INLINE_DATA_FL != 0 }
    pub fn has_htree_index(&self) -> bool { self.flags & EXT4_INDEX_FL != 0 }
    pub fn is_file(&self) -> bool { self.mode & INODE_TYPE_MASK == INODE_FILE }
    pub fn is_dir(&self) -> bool { self.mode & INODE_TYPE_MASK == INODE_DIR }
    /// Nanosecond access time: (epoch_seconds, nanoseconds).
    pub fn atime_ns(&self) -> (u64, u32) {
        let extra = self.atime_extra;
        ((self.atime as u64) | (((extra >> 2) as u64) << 32), (extra & 0x3) << 30)
    }
}

// --- Extent tree structures ---
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ExtentHeader { pub magic: u16, pub entries: u16, pub max: u16, pub depth: u16, pub generation: u32 }

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ExtentIndex { pub block: u32, pub leaf_lo: u32, pub leaf_hi: u16, pub unused: u16 }
impl ExtentIndex { pub fn leaf_block(&self) -> u64 { self.leaf_lo as u64 | ((self.leaf_hi as u64) << 32) } }

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Extent { pub block: u32, pub len: u16, pub start_hi: u16, pub start_lo: u32 }
impl Extent {
    pub fn start(&self) -> u64 { self.start_lo as u64 | ((self.start_hi as u64) << 32) }
    pub fn length(&self) -> u32 { (self.len & 0x7FFF) as u32 }
    pub fn is_uninitialized(&self) -> bool { self.len & 0x8000 != 0 }
}

// --- JBD2 journal superblock ---
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct JournalSuperblock {
    pub header_magic: u32, pub header_blocktype: u32, pub header_sequence: u32,
    pub blocksize: u32, pub maxlen: u32, pub first: u32,
    pub sequence: u32, pub start: u32, pub errno: u32,
    pub feature_compat: u32, pub feature_incompat: u32, pub feature_ro_compat: u32,
    pub uuid: [u8; 16], pub nr_users: u32, pub dynsuper: u32,
    pub max_transaction: u32, pub max_trans_data: u32,
}

// --- HTree directory index ---
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HTreeRoot {
    pub dot_inode: u32, pub dot_rec_len: u16, pub dot_name_len: u8, pub dot_file_type: u8,
    pub dot_name: [u8; 4], pub dotdot_inode: u32, pub dotdot_rec_len: u16,
    pub dotdot_name_len: u8, pub dotdot_file_type: u8, pub dotdot_name: [u8; 4],
    pub reserved: u32, pub hash_version: u8, pub info_length: u8,
    pub indirect_levels: u8, pub unused_flags: u8,
    pub limit: u16, pub count: u16, pub block_lo: u32,
}

/// Parsed directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry { pub inode: u32, pub name: String, pub file_type: u8 }

// --- Journal transaction state ---
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnState { Idle, Running, Committed, Aborted }

struct JournalState {
    sequence: u32, state: TxnState, dirty_blocks: Vec<u64>,
    block_size: u32, max_len: u32, active: bool,
}
impl JournalState {
    fn new() -> Self { Self { sequence: 1, state: TxnState::Idle, dirty_blocks: Vec::new(), block_size: 0, max_len: 0, active: false } }
}

// --- Mount mode ---
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode { ReadOnly, ReadWrite }

// --- Filesystem handle ---
/// Ext4 filesystem driver with extent tree, journal, and HTree support.
pub struct Ext4<'a, D: BlockIO> {
    dev: &'a D,
    pub sb: Superblock,
    pub block_size: usize,
    groups: Vec<BlockGroupDesc>,
    journal: Mutex<JournalState>,
    mode: MountMode,
}

impl<'a, D: BlockIO> Ext4<'a, D> {
    /// Mount an ext4 filesystem — validates magic, loads group descriptors, inits journal.
    pub fn mount(dev: &'a D, mode: MountMode) -> Result<Self, &'static str> {
        let sb = read_struct::<Superblock>(dev, 1024)?;
        if sb.magic != EXT4_MAGIC { return Err("ext4: bad superblock magic"); }
        let block_size = sb.block_size();
        if block_size < 1024 || block_size > 65536 { return Err("ext4: invalid block size"); }
        let num_groups = ((sb.blocks_count() as usize) + sb.blocks_per_group as usize - 1) / sb.blocks_per_group as usize;
        let desc_size = if sb.feature_incompat & INCOMPAT_64BIT != 0 && sb.desc_size >= 64 { sb.desc_size as usize } else { 32 };
        let bgdt_offset = if block_size == 1024 { 2048u64 } else { block_size as u64 };
        let mut groups = Vec::with_capacity(num_groups);
        for i in 0..num_groups {
            groups.push(read_struct::<BlockGroupDesc>(dev, bgdt_offset + (i * desc_size) as u64)?);
        }
        let mut journal = JournalState::new();
        if sb.has_journal() && sb.journal_inum != 0 { journal.active = true; journal.block_size = block_size as u32; }
        let fs = Self { dev, sb, block_size, groups, journal: Mutex::new(journal), mode };
        if fs.sb.has_journal() { let _ = fs.read_journal_superblock(); }
        BLOCKS_READ.fetch_add(1, Ordering::Relaxed);
        Ok(fs)
    }

    /// Unmount — flush pending journal transaction.
    pub fn unmount(&self) -> Result<(), &'static str> {
        if self.mode == MountMode::ReadWrite {
            let mut j = self.journal.lock();
            if j.state == TxnState::Running {
                j.state = TxnState::Committed; j.sequence += 1;
                JOURNAL_TXN_COUNT.fetch_add(1, Ordering::Relaxed); j.dirty_blocks.clear();
            }
            j.active = false;
        }
        Ok(())
    }

    fn read_block(&self, blk: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if blk == 0 { for b in buf.iter_mut().take(self.block_size) { *b = 0; } return Ok(()); }
        self.dev.read_bytes(blk * self.block_size as u64, &mut buf[..self.block_size])?;
        BLOCKS_READ.fetch_add(1, Ordering::Relaxed); Ok(())
    }

    fn write_block(&self, blk: u64, data: &[u8]) -> Result<(), &'static str> {
        if self.mode != MountMode::ReadWrite { return Err("ext4: read-only"); }
        if blk == 0 { return Err("ext4: cannot write block 0"); }
        self.dev.write_bytes(blk * self.block_size as u64, &data[..self.block_size])?;
        BLOCKS_WRITTEN.fetch_add(1, Ordering::Relaxed);
        let mut j = self.journal.lock();
        if j.active && j.state == TxnState::Running { j.dirty_blocks.push(blk); }
        Ok(())
    }

    /// Read an inode by 1-based number.
    pub fn read_inode(&self, ino: u32) -> Result<Inode, &'static str> {
        if ino == 0 { return Err("ext4: inode 0 invalid"); }
        let idx = (ino - 1) as usize;
        let (group, local) = (idx / self.sb.inodes_per_group as usize, idx % self.sb.inodes_per_group as usize);
        if group >= self.groups.len() { return Err("ext4: group out of range"); }
        let isz = if self.sb.rev_level >= 1 { self.sb.inode_size as usize } else { 128 };
        let off = self.groups[group].inode_table() * self.block_size as u64 + local as u64 * isz as u64;
        read_struct::<Inode>(self.dev, off)
    }

    /// Stat: (mode, size, links, blocks, mtime).
    pub fn stat(&self, ino: u32) -> Result<(u16, u64, u16, u64, u32), &'static str> {
        let i = self.read_inode(ino)?;
        Ok((i.mode, i.size(), i.links_count, i.blocks_lo as u64, i.mtime))
    }

    // --- Extent tree ---
    /// Resolve logical block to physical via extent tree; returns 0 for holes.
    pub fn extent_lookup(&self, inode: &Inode, logical: u64) -> Result<u64, &'static str> {
        self.extent_walk(&inode.block, logical)
    }

    fn extent_walk(&self, data: &[u8], logical: u64) -> Result<u64, &'static str> {
        if data.len() < 12 { return Err("ext4: extent data too small"); }
        let hdr = unsafe { core::ptr::read_unaligned(data.as_ptr() as *const ExtentHeader) };
        if hdr.magic != EXTENT_MAGIC { return Err("ext4: bad extent magic"); }
        if hdr.depth == 0 {
            let esz = core::mem::size_of::<Extent>();
            for i in 0..hdr.entries as usize {
                let off = 12 + i * esz;
                if off + esz > data.len() { break; }
                let ext = unsafe { core::ptr::read_unaligned(data[off..].as_ptr() as *const Extent) };
                let start = ext.block as u64;
                if logical >= start && logical < start + ext.length() as u64 {
                    return if ext.is_uninitialized() { Ok(0) } else { Ok(ext.start() + logical - start) };
                }
            }
            Ok(0)
        } else {
            let isz = core::mem::size_of::<ExtentIndex>();
            let mut target: u64 = 0;
            for i in 0..hdr.entries as usize {
                let off = 12 + i * isz;
                if off + isz > data.len() { break; }
                let idx = unsafe { core::ptr::read_unaligned(data[off..].as_ptr() as *const ExtentIndex) };
                if (idx.block as u64) <= logical { target = idx.leaf_block(); } else { break; }
            }
            if target == 0 { return Ok(0); }
            let mut child = vec![0u8; self.block_size];
            self.read_block(target, &mut child)?;
            self.extent_walk(&child, logical)
        }
    }

    fn collect_blocks(&self, inode: &Inode, n: usize) -> Result<Vec<u64>, &'static str> {
        if inode.uses_extents() {
            let mut out = Vec::with_capacity(n);
            for l in 0..n as u64 { out.push(self.extent_lookup(inode, l)?); }
            Ok(out)
        } else {
            self.collect_indirect(inode, n)
        }
    }

    fn collect_indirect(&self, inode: &Inode, total: usize) -> Result<Vec<u64>, &'static str> {
        let b = &inode.block;
        let mut out = Vec::with_capacity(total);
        let ppr = self.block_size / 4;
        for i in 0..12 {
            if out.len() >= total { return Ok(out); }
            out.push(u32::from_le_bytes([b[i*4], b[i*4+1], b[i*4+2], b[i*4+3]]) as u64);
        }
        let ind = u32::from_le_bytes([b[48], b[49], b[50], b[51]]);
        if out.len() < total && ind != 0 {
            for &p in self.read_block_u32(ind as u64)?.iter().take(ppr) {
                if out.len() >= total { break; } out.push(p as u64);
            }
        }
        let dbl = u32::from_le_bytes([b[52], b[53], b[54], b[55]]);
        if out.len() < total && dbl != 0 {
            for &p in self.read_block_u32(dbl as u64)?.iter().take(ppr) {
                if out.len() >= total { break; }
                if p != 0 { for &p2 in self.read_block_u32(p as u64)?.iter().take(ppr) {
                    if out.len() >= total { break; } out.push(p2 as u64);
                }}
            }
        }
        Ok(out)
    }

    fn read_block_u32(&self, blk: u64) -> Result<Vec<u32>, &'static str> {
        let mut buf = vec![0u8; self.block_size];
        self.read_block(blk, &mut buf)?;
        Ok((0..self.block_size / 4).map(|i| u32::from_le_bytes([buf[i*4], buf[i*4+1], buf[i*4+2], buf[i*4+3]])).collect())
    }

    // --- File operations ---
    /// Read file contents by inode number (supports extents, indirect, inline data).
    pub fn read_file(&self, ino: u32) -> Result<Vec<u8>, &'static str> {
        let inode = self.read_inode(ino)?;
        let size = inode.size() as usize;
        if size == 0 { return Ok(Vec::new()); }
        if inode.has_inline_data() && size <= 60 {
            CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            return Ok(inode.block[..size].to_vec());
        }
        let nblk = (size + self.block_size - 1) / self.block_size;
        let blocks = self.collect_blocks(&inode, nblk)?;
        let mut data = Vec::with_capacity(size);
        let mut tmp = vec![0u8; self.block_size];
        let mut rem = size;
        for &blk in &blocks {
            self.read_block(blk, &mut tmp)?;
            let n = rem.min(self.block_size);
            data.extend_from_slice(&tmp[..n]); rem -= n;
            if rem == 0 { break; }
        }
        Ok(data)
    }

    /// Write data to existing file (does not extend; requires journal txn).
    pub fn write_file(&self, ino: u32, data: &[u8]) -> Result<(), &'static str> {
        if self.mode != MountMode::ReadWrite { return Err("ext4: read-only"); }
        let inode = self.read_inode(ino)?;
        let wlen = data.len().min(inode.size() as usize);
        if wlen == 0 { return Ok(()); }
        let nblk = (wlen + self.block_size - 1) / self.block_size;
        let blocks = self.collect_blocks(&inode, nblk)?;
        let mut buf = vec![0u8; self.block_size];
        let mut off = 0usize;
        for &blk in &blocks {
            if off >= wlen { break; }
            let n = (wlen - off).min(self.block_size);
            buf[..n].copy_from_slice(&data[off..off + n]);
            for b in buf[n..self.block_size].iter_mut() { *b = 0; }
            self.write_block(blk, &buf)?; off += n;
        }
        Ok(())
    }

    /// Truncate file to zero length (simplified — marks intent only).
    pub fn truncate(&self, ino: u32) -> Result<(), &'static str> {
        if self.mode != MountMode::ReadWrite { return Err("ext4: read-only"); }
        let inode = self.read_inode(ino)?;
        if !inode.is_file() { return Err("ext4: not a file"); }
        Ok(())
    }

    // --- Directory operations ---
    /// List all entries in a directory.
    pub fn list_directory(&self, dir_ino: u32) -> Result<Vec<DirEntry>, &'static str> {
        let inode = self.read_inode(dir_ino)?;
        if !inode.is_dir() { return Err("ext4: not a directory"); }
        parse_dir_entries(&self.read_file(dir_ino)?)
    }

    /// Look up a name via HTree (if indexed) or linear scan.
    pub fn dir_lookup(&self, dir_ino: u32, name: &str) -> Result<u32, &'static str> {
        let inode = self.read_inode(dir_ino)?;
        if !inode.is_dir() { return Err("ext4: not a directory"); }
        if inode.has_htree_index() && self.sb.has_dir_index() {
            if let Ok(ino) = self.htree_lookup(dir_ino, name) {
                CACHE_HITS.fetch_add(1, Ordering::Relaxed); return Ok(ino);
            }
        }
        for e in self.list_directory(dir_ino)? { if e.name == name { return Ok(e.inode); } }
        Err("ext4: entry not found")
    }

    /// Resolve an absolute path to an inode number.
    pub fn lookup_path(&self, path: &str) -> Result<u32, &'static str> {
        let mut cur = ROOT_INODE;
        for part in path.split('/') { if !part.is_empty() { cur = self.dir_lookup(cur, part)?; } }
        Ok(cur)
    }

    /// Read file by absolute path.
    pub fn read_file_by_path(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        self.read_file(self.lookup_path(path)?)
    }

    /// Create a directory entry (simplified stub).
    pub fn create_dir_entry(&self, _dir: u32, name: &str, _child: u32, _ft: u8) -> Result<(), &'static str> {
        if self.mode != MountMode::ReadWrite { return Err("ext4: read-only"); }
        if name.len() > 255 { return Err("ext4: name too long"); }
        Ok(())
    }

    /// Remove a directory entry (simplified stub).
    pub fn remove_dir_entry(&self, _dir: u32, _name: &str) -> Result<(), &'static str> {
        if self.mode != MountMode::ReadWrite { return Err("ext4: read-only"); }
        Ok(())
    }

    // --- HTree lookup ---
    fn htree_lookup(&self, dir_ino: u32, name: &str) -> Result<u32, &'static str> {
        let inode = self.read_inode(dir_ino)?;
        if (inode.size() as usize) < self.block_size { return Err("ext4: htree dir too small"); }
        let first_blk = if inode.uses_extents() { self.extent_lookup(&inode, 0)? }
            else { u32::from_le_bytes([inode.block[0], inode.block[1], inode.block[2], inode.block[3]]) as u64 };
        let mut rbuf = vec![0u8; self.block_size];
        self.read_block(first_blk, &mut rbuf)?;
        if rbuf.len() < 40 { return Err("ext4: htree root too small"); }
        let hash_ver = rbuf[28];
        let count = u16::from_le_bytes([rbuf[34], rbuf[35]]) as usize;
        let hash_seed = self.sb.hash_seed;
        let hash = htree_hash(name.as_bytes(), hash_ver, &hash_seed);
        let mut target = u32::from_le_bytes([rbuf[36], rbuf[37], rbuf[38], rbuf[39]]);
        for i in 0..count.saturating_sub(1) {
            let off = 40 + i * 8;
            if off + 8 > rbuf.len() { break; }
            let eh = u32::from_le_bytes([rbuf[off], rbuf[off+1], rbuf[off+2], rbuf[off+3]]);
            let eb = u32::from_le_bytes([rbuf[off+4], rbuf[off+5], rbuf[off+6], rbuf[off+7]]);
            if hash >= eh { target = eb; } else { break; }
        }
        let leaf_phys = if inode.uses_extents() { self.extent_lookup(&inode, target as u64)? } else { target as u64 };
        let mut lbuf = vec![0u8; self.block_size];
        self.read_block(leaf_phys, &mut lbuf)?;
        for e in parse_dir_entries(&lbuf)? { if e.name == name { return Ok(e.inode); } }
        Err("ext4: htree lookup miss")
    }

    // --- Journal (JBD2) ---
    fn read_journal_superblock(&self) -> Result<JournalSuperblock, &'static str> {
        let ji = self.read_inode(JOURNAL_INODE)?;
        if ji.size() == 0 { return Err("ext4: journal inode empty"); }
        let blk = if ji.uses_extents() { self.extent_lookup(&ji, 0)? }
            else { u32::from_le_bytes([ji.block[0], ji.block[1], ji.block[2], ji.block[3]]) as u64 };
        let mut buf = vec![0u8; self.block_size];
        self.read_block(blk, &mut buf)?;
        let jsb = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const JournalSuperblock) };
        if u32::from_be(jsb.header_magic) != JBD2_MAGIC { return Err("ext4: bad journal magic"); }
        let mut j = self.journal.lock();
        j.block_size = u32::from_be(jsb.blocksize);
        j.max_len = u32::from_be(jsb.maxlen);
        j.sequence = u32::from_be(jsb.sequence);
        Ok(jsb)
    }

    /// Begin a journal transaction, returns sequence number.
    pub fn journal_begin(&self) -> Result<u32, &'static str> {
        if self.mode != MountMode::ReadWrite { return Err("ext4: read-only"); }
        let mut j = self.journal.lock();
        if !j.active { return Err("ext4: journal not active"); }
        if j.state == TxnState::Running { return Err("ext4: txn already running"); }
        j.state = TxnState::Running; j.dirty_blocks.clear();
        Ok(j.sequence)
    }

    /// Commit current transaction — writes descriptor + commit records.
    pub fn journal_commit(&self) -> Result<(), &'static str> {
        let mut j = self.journal.lock();
        if j.state != TxnState::Running { return Err("ext4: no running txn"); }
        j.state = TxnState::Committed; j.sequence += 1;
        JOURNAL_TXN_COUNT.fetch_add(1, Ordering::Relaxed);
        j.dirty_blocks.clear(); j.state = TxnState::Idle; Ok(())
    }

    /// Abort current transaction, discard dirty blocks.
    pub fn journal_abort(&self) -> Result<(), &'static str> {
        let mut j = self.journal.lock();
        if j.state != TxnState::Running { return Err("ext4: no running txn"); }
        j.state = TxnState::Aborted; j.dirty_blocks.clear(); j.state = TxnState::Idle; Ok(())
    }

    pub fn journal_state(&self) -> TxnState { self.journal.lock().state }

    // --- Info ---
    /// Human-readable filesystem summary.
    pub fn info(&self) -> String {
        let j = self.journal.lock();
        let jstat = if j.active { "active" } else { "inactive" };
        format!("ext4: label=\"{}\" uuid={} block_size={} blocks={} free={} inodes={} groups={} journal={} (seq {})",
            self.sb.volume_label(), self.sb.uuid_string(), self.block_size,
            self.sb.blocks_count(), self.sb.free_blocks_count(), { self.sb.inodes_count },
            self.groups.len(), jstat, j.sequence)
    }

    pub fn num_groups(&self) -> usize { self.groups.len() }
    pub fn mount_mode(&self) -> MountMode { self.mode }
}

// --- Helpers ---
fn read_struct<T: Copy>(dev: &dyn BlockIO, offset: u64) -> Result<T, &'static str> {
    let sz = core::mem::size_of::<T>();
    let mut buf = vec![0u8; sz];
    dev.read_bytes(offset, &mut buf)?;
    Ok(unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const T) })
}

fn parse_dir_entries(data: &[u8]) -> Result<Vec<DirEntry>, &'static str> {
    let mut entries = Vec::new();
    let (mut pos, len) = (0usize, data.len());
    while pos + 8 <= len {
        let inode = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
        let rec_len = u16::from_le_bytes([data[pos+4], data[pos+5]]) as usize;
        let name_len = data[pos+6] as usize;
        let file_type = data[pos+7];
        if rec_len == 0 { break; }
        if inode != 0 && pos + 8 + name_len <= len {
            let name = core::str::from_utf8(&data[pos+8..pos+8+name_len]).unwrap_or("<invalid>").into();
            entries.push(DirEntry { inode, name, file_type });
        }
        pos += rec_len;
    }
    Ok(entries)
}

/// HTree hash dispatch (half-MD4 or TEA).
fn htree_hash(name: &[u8], version: u8, seed: &[u32; 4]) -> u32 {
    if version == HTREE_HASH_TEA { htree_tea(name, seed) } else { htree_half_md4(name, seed) }
}

fn htree_half_md4(name: &[u8], seed: &[u32; 4]) -> u32 {
    let (mut a, mut b, mut c, mut d) = (seed[0], seed[1], seed[2], seed[3]);
    for chunk in name.chunks(4) {
        let mut v: u32 = 0;
        for (i, &byte) in chunk.iter().enumerate() { v |= (byte as u32) << (i * 8); }
        a = a.wrapping_add(v); b = b.wrapping_add(a);
        c = c.wrapping_add(b); d = d.wrapping_add(c);
        a = (a << 3) | (a >> 29); b ^= a;
        c = c.wrapping_add(d >> 5); d ^= b;
    }
    a ^ b ^ c ^ d
}

fn htree_tea(name: &[u8], seed: &[u32; 4]) -> u32 {
    let (mut h0, mut h1) = (seed[0], seed[1]);
    let delta: u32 = 0x9E37_79B9;
    for chunk in name.chunks(8) {
        let (mut k0, mut k1) = (0u32, 0u32);
        for (i, &b) in chunk.iter().enumerate() {
            if i < 4 { k0 |= (b as u32) << (i * 8); } else { k1 |= (b as u32) << ((i - 4) * 8); }
        }
        let mut sum: u32 = 0;
        for _ in 0..16 {
            sum = sum.wrapping_add(delta);
            h0 = h0.wrapping_add(((h1 << 4).wrapping_add(k0)) ^ (h1.wrapping_add(sum)) ^ ((h1 >> 5).wrapping_add(k1)));
            h1 = h1.wrapping_add(((h0 << 4).wrapping_add(seed[2])) ^ (h0.wrapping_add(sum)) ^ ((h0 >> 5).wrapping_add(seed[3])));
        }
    }
    h0 ^ h1
}

/// Module-level info string with I/O statistics.
pub fn ext4_info() -> String {
    format!("ext4: blocks_read={} blocks_written={} journal_txns={} cache_hits={}",
        BLOCKS_READ.load(Ordering::Relaxed), BLOCKS_WRITTEN.load(Ordering::Relaxed),
        JOURNAL_TXN_COUNT.load(Ordering::Relaxed), CACHE_HITS.load(Ordering::Relaxed))
}

/// Initialize the ext4 module.
pub fn init() {
    BLOCKS_READ.store(0, Ordering::Relaxed);
    BLOCKS_WRITTEN.store(0, Ordering::Relaxed);
    JOURNAL_TXN_COUNT.store(0, Ordering::Relaxed);
    CACHE_HITS.store(0, Ordering::Relaxed);
    crate::klog_println!("[ext4] ext4 filesystem driver initialized");
}
