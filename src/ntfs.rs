/// NTFS read-only filesystem driver for MerlionOS.
/// Parses NTFS volumes for reading files from Windows drives.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// --- Statistics ---
static MFT_READS: AtomicU64 = AtomicU64::new(0);
static DATA_READS: AtomicU64 = AtomicU64::new(0);
static DIR_LOOKUPS: AtomicU64 = AtomicU64::new(0);
static BYTES_READ: AtomicU64 = AtomicU64::new(0);

// --- Constants ---
/// NTFS OEM ID in boot sector.
const NTFS_OEM_ID: &[u8; 8] = b"NTFS    ";

/// MFT entry signature "FILE".
const FILE_SIGNATURE: u32 = 0x454C_4946;

/// Attribute type IDs.
const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_INDEX_ROOT: u32 = 0x90;
const ATTR_INDEX_ALLOCATION: u32 = 0xA0;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Well-known MFT entry numbers.
const MFT_ENTRY_MFT: u64 = 0;
const MFT_ENTRY_MFTMIRR: u64 = 1;
const MFT_ENTRY_LOGFILE: u64 = 2;
const MFT_ENTRY_VOLUME: u64 = 3;
const MFT_ENTRY_ROOT: u64 = 5;

/// File attribute flags.
const FILE_ATTR_DIRECTORY: u32 = 0x1000_0000;
const FILE_ATTR_READONLY: u32 = 0x0001;
const FILE_ATTR_HIDDEN: u32 = 0x0002;
const FILE_ATTR_SYSTEM: u32 = 0x0004;

/// Index entry flags.
const INDEX_ENTRY_SUBNODE: u32 = 0x01;
const INDEX_ENTRY_LAST: u32 = 0x02;

// --- NTFS Boot Sector (BPB) ---
/// NTFS BIOS Parameter Block parsed from the first sector.
#[derive(Debug, Clone, Copy)]
pub struct NtfsBpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub total_sectors: u64,
    pub mft_cluster: u64,
    pub mft_mirror_cluster: u64,
    pub mft_record_size: u32,
    pub index_record_size: u32,
    pub serial_number: u64,
}

impl NtfsBpb {
    /// Parse BPB from a 512-byte boot sector buffer.
    pub fn parse(sector: &[u8]) -> Result<Self, &'static str> {
        if sector.len() < 512 {
            return Err("boot sector too small");
        }
        // Check OEM ID at offset 3
        let oem = &sector[3..11];
        if oem != NTFS_OEM_ID {
            return Err("not an NTFS volume (bad OEM ID)");
        }

        let bytes_per_sector = read_u16(sector, 11);
        let sectors_per_cluster = sector[13];
        let total_sectors = read_u64(sector, 40);
        let mft_cluster = read_u64(sector, 48);
        let mft_mirror_cluster = read_u64(sector, 56);

        // MFT record size: if value < 0, size = 2^|value|; else clusters
        let mft_record_size = decode_cluster_or_log2(sector[64], bytes_per_sector, sectors_per_cluster);
        let index_record_size = decode_cluster_or_log2(sector[68], bytes_per_sector, sectors_per_cluster);
        let serial_number = read_u64(sector, 72);

        if bytes_per_sector == 0 || sectors_per_cluster == 0 {
            return Err("invalid BPB values");
        }

        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            total_sectors,
            mft_cluster,
            mft_mirror_cluster,
            mft_record_size,
            index_record_size,
            serial_number,
        })
    }

    /// Bytes per cluster.
    pub fn cluster_size(&self) -> u32 {
        self.bytes_per_sector as u32 * self.sectors_per_cluster as u32
    }

    /// Convert cluster number to byte offset on volume.
    pub fn cluster_to_offset(&self, cluster: u64) -> u64 {
        cluster * self.cluster_size() as u64
    }
}

/// Decode a clusters-or-log2 size field.
fn decode_cluster_or_log2(val: u8, bytes_per_sector: u16, sectors_per_cluster: u8) -> u32 {
    if val == 0 {
        return 0;
    }
    // If high bit set, it's a signed power of 2
    if val & 0x80 != 0 {
        let shift = (256u32).wrapping_sub(val as u32) & 0xFF;
        if shift < 32 { 1u32 << shift } else { 0 }
    } else {
        val as u32 * sectors_per_cluster as u32 * bytes_per_sector as u32
    }
}

// --- MFT Entry ---
/// Header of an MFT entry (FILE record).
#[derive(Debug, Clone, Copy)]
pub struct MftEntryHeader {
    pub signature: u32,
    pub fixup_offset: u16,
    pub fixup_count: u16,
    pub lsn: u64,
    pub sequence_number: u16,
    pub link_count: u16,
    pub first_attr_offset: u16,
    pub flags: u16,
    pub used_size: u32,
    pub allocated_size: u32,
    pub base_record: u64,
}

impl MftEntryHeader {
    /// Parse from buffer (at least 48 bytes).
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 48 {
            return Err("MFT entry too small");
        }
        let sig = read_u32(data, 0);
        if sig != FILE_SIGNATURE {
            return Err("bad MFT signature (not FILE)");
        }
        Ok(Self {
            signature: sig,
            fixup_offset: read_u16(data, 4),
            fixup_count: read_u16(data, 6),
            lsn: read_u64(data, 8),
            sequence_number: read_u16(data, 16),
            link_count: read_u16(data, 18),
            first_attr_offset: read_u16(data, 20),
            flags: read_u16(data, 22),
            used_size: read_u32(data, 24),
            allocated_size: read_u32(data, 28),
            base_record: read_u64(data, 32),
        })
    }

    pub fn is_in_use(&self) -> bool {
        self.flags & 0x01 != 0
    }

    pub fn is_directory(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

/// Apply fixup array to an MFT record buffer.
pub fn apply_fixups(buf: &mut [u8], fixup_offset: u16, fixup_count: u16, sector_size: u16) {
    if fixup_count < 2 || fixup_offset as usize + (fixup_count as usize) * 2 > buf.len() {
        return;
    }
    let off = fixup_offset as usize;
    let signature = read_u16(buf, off);
    for i in 1..fixup_count as usize {
        let sector_end = i * sector_size as usize - 2;
        if sector_end + 2 <= buf.len() && off + i * 2 + 1 < buf.len() {
            // Verify fixup signature matches
            let stored = read_u16(buf, sector_end);
            if stored == signature {
                buf[sector_end] = buf[off + i * 2];
                buf[sector_end + 1] = buf[off + i * 2 + 1];
            }
        }
    }
}

// --- Attributes ---
/// Generic NTFS attribute header.
#[derive(Debug, Clone, Copy)]
pub struct AttrHeader {
    pub attr_type: u32,
    pub length: u32,
    pub non_resident: bool,
    pub name_length: u8,
    pub name_offset: u16,
    pub flags: u16,
    pub instance: u16,
}

impl AttrHeader {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 16 {
            return Err("attribute header too small");
        }
        let attr_type = read_u32(data, 0);
        if attr_type == ATTR_END {
            return Err("end of attributes");
        }
        Ok(Self {
            attr_type,
            length: read_u32(data, 4),
            non_resident: data[8] != 0,
            name_length: data[9],
            name_offset: read_u16(data, 10),
            flags: read_u16(data, 12),
            instance: read_u16(data, 14),
        })
    }
}

/// Resident attribute data.
#[derive(Debug, Clone)]
pub struct ResidentData {
    pub value_length: u32,
    pub value_offset: u16,
}

impl ResidentData {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 24 {
            return Err("resident attr too small");
        }
        Ok(Self {
            value_length: read_u32(data, 16),
            value_offset: read_u16(data, 20),
        })
    }

    /// Get the value bytes from the full attribute buffer.
    pub fn value<'a>(&self, attr_data: &'a [u8]) -> &'a [u8] {
        let start = self.value_offset as usize;
        let end = start + self.value_length as usize;
        if end <= attr_data.len() {
            &attr_data[start..end]
        } else {
            &[]
        }
    }
}

/// Non-resident attribute header fields.
#[derive(Debug, Clone, Copy)]
pub struct NonResidentData {
    pub lowest_vcn: u64,
    pub highest_vcn: u64,
    pub data_runs_offset: u16,
    pub compression_unit: u16,
    pub allocated_size: u64,
    pub real_size: u64,
    pub initialized_size: u64,
}

impl NonResidentData {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 72 {
            return Err("non-resident attr too small");
        }
        Ok(Self {
            lowest_vcn: read_u64(data, 16),
            highest_vcn: read_u64(data, 24),
            data_runs_offset: read_u16(data, 32),
            compression_unit: read_u16(data, 34),
            allocated_size: read_u64(data, 40),
            real_size: read_u64(data, 48),
            initialized_size: read_u64(data, 56),
        })
    }
}

// --- Data Runs ---
/// A single data run (extent): cluster offset + length.
#[derive(Debug, Clone, Copy)]
pub struct DataRun {
    /// Starting cluster (absolute).
    pub cluster: u64,
    /// Number of clusters in this run.
    pub length: u64,
}

/// Parse NTFS compressed data runs from a byte slice.
/// Each run is encoded as: header byte (low nibble = length_size, high nibble = offset_size),
/// followed by length bytes, then offset bytes (signed, relative to previous run).
pub fn parse_data_runs(data: &[u8]) -> Vec<DataRun> {
    let mut runs = Vec::new();
    let mut pos = 0;
    let mut prev_cluster: i64 = 0;

    while pos < data.len() {
        let header = data[pos];
        if header == 0 {
            break;
        }
        pos += 1;

        let length_size = (header & 0x0F) as usize;
        let offset_size = ((header >> 4) & 0x0F) as usize;

        if length_size == 0 || pos + length_size + offset_size > data.len() {
            break;
        }

        // Read length (unsigned)
        let mut run_length: u64 = 0;
        for i in 0..length_size {
            run_length |= (data[pos + i] as u64) << (i * 8);
        }
        pos += length_size;

        // Read offset (signed, relative)
        if offset_size == 0 {
            // Sparse run
            runs.push(DataRun { cluster: 0, length: run_length });
        } else {
            let mut offset: i64 = 0;
            for i in 0..offset_size {
                offset |= (data[pos + i] as i64) << (i * 8);
            }
            // Sign extend
            if offset_size < 8 && data[pos + offset_size - 1] & 0x80 != 0 {
                for i in offset_size..8 {
                    offset |= 0xFFi64 << (i * 8);
                }
            }
            pos += offset_size;

            prev_cluster += offset;
            if prev_cluster >= 0 {
                runs.push(DataRun {
                    cluster: prev_cluster as u64,
                    length: run_length,
                });
            }
        }
    }
    runs
}

// --- File Name Attribute ---
/// Parsed $FILE_NAME attribute.
#[derive(Debug, Clone)]
pub struct FileName {
    pub parent_ref: u64,
    pub name: String,
    pub flags: u32,
    pub real_size: u64,
    pub namespace: u8,
}

impl FileName {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 66 {
            return Err("FILE_NAME attr too small");
        }
        let parent_ref = read_u48(data, 0);
        let real_size = read_u64(data, 48);
        let flags = read_u32(data, 56);
        let name_length = data[64] as usize;
        let namespace = data[65];

        // Name is UTF-16LE starting at offset 66
        let name = decode_utf16le(data, 66, name_length);

        Ok(Self {
            parent_ref,
            name,
            flags,
            real_size,
            namespace,
        })
    }

    pub fn is_directory(&self) -> bool {
        self.flags & FILE_ATTR_DIRECTORY != 0
    }
}

// --- Index Entry ---
/// An entry in a directory index (B-tree node).
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub mft_reference: u64,
    pub entry_length: u16,
    pub content_length: u16,
    pub flags: u32,
    pub file_name: Option<FileName>,
}

impl IndexEntry {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 16 {
            return Err("index entry too small");
        }
        let mft_reference = read_u48(data, 0);
        let entry_length = read_u16(data, 8);
        let content_length = read_u16(data, 10);
        let flags = read_u32(data, 12);

        let file_name = if content_length >= 66 && data.len() >= 16 + content_length as usize {
            FileName::parse(&data[16..]).ok()
        } else {
            None
        };

        Ok(Self {
            mft_reference,
            entry_length,
            content_length,
            flags,
            file_name,
        })
    }

    pub fn is_last(&self) -> bool {
        self.flags & INDEX_ENTRY_LAST != 0
    }

    pub fn has_subnode(&self) -> bool {
        self.flags & INDEX_ENTRY_SUBNODE != 0
    }
}

// --- Volume State ---
/// Represents a mounted NTFS volume.
pub struct NtfsVolume {
    pub bpb: NtfsBpb,
    pub volume_label: String,
    /// Raw bytes of the volume (simulated device).
    data: Vec<u8>,
}

impl NtfsVolume {
    /// Read bytes from the volume at a given offset.
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let start = offset as usize;
        let end = start + buf.len();
        if end > self.data.len() {
            return Err("read past end of volume");
        }
        buf.copy_from_slice(&self.data[start..end]);
        DATA_READS.fetch_add(1, Ordering::Relaxed);
        BYTES_READ.fetch_add(buf.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Read an MFT entry by index.
    pub fn read_mft_entry(&self, index: u64) -> Result<Vec<u8>, &'static str> {
        let mft_offset = self.bpb.cluster_to_offset(self.bpb.mft_cluster);
        let entry_offset = mft_offset + index * self.bpb.mft_record_size as u64;
        let mut buf = vec![0u8; self.bpb.mft_record_size as usize];
        self.read_bytes(entry_offset, &mut buf)?;
        let fixup_offset = read_u16(&buf, 4);
        let fixup_count = read_u16(&buf, 6);
        apply_fixups(
            &mut buf,
            fixup_offset,
            fixup_count,
            self.bpb.bytes_per_sector,
        );
        MFT_READS.fetch_add(1, Ordering::Relaxed);
        Ok(buf)
    }

    /// Find an attribute of the given type in an MFT entry buffer.
    pub fn find_attribute(&self, entry: &[u8], attr_type: u32) -> Option<(AttrHeader, usize)> {
        let header = MftEntryHeader::parse(entry).ok()?;
        let mut offset = header.first_attr_offset as usize;
        while offset + 16 <= entry.len() {
            let t = read_u32(entry, offset);
            if t == ATTR_END || t == 0 {
                break;
            }
            let len = read_u32(entry, offset + 4) as usize;
            if len == 0 || offset + len > entry.len() {
                break;
            }
            if t == attr_type {
                if let Ok(ah) = AttrHeader::parse(&entry[offset..]) {
                    return Some((ah, offset));
                }
            }
            offset += len;
        }
        None
    }

    /// Get the file name from an MFT entry.
    pub fn get_file_name(&self, entry: &[u8]) -> Option<FileName> {
        let (ah, offset) = self.find_attribute(entry, ATTR_FILE_NAME)?;
        if ah.non_resident {
            return None; // FILE_NAME is always resident
        }
        let rd = ResidentData::parse(&entry[offset..]).ok()?
            .value(&entry[offset..]);
        if !rd.is_empty() {
            return None;
        }
        // Actually parse from the resident value
        let res = ResidentData::parse(&entry[offset..]).ok()?;
        let value = res.value(&entry[offset..]);
        FileName::parse(value).ok()
    }

    /// Read resident attribute data.
    pub fn read_resident_data(&self, entry: &[u8], attr_type: u32) -> Option<Vec<u8>> {
        let (_ah, offset) = self.find_attribute(entry, attr_type)?;
        let res = ResidentData::parse(&entry[offset..]).ok()?;
        let value = res.value(&entry[offset..]);
        Some(value.to_vec())
    }

    /// Read non-resident attribute data via data runs.
    pub fn read_nonresident_data(&self, entry: &[u8], attr_type: u32) -> Option<Vec<u8>> {
        let (ah, offset) = self.find_attribute(entry, attr_type)?;
        if !ah.non_resident {
            return self.read_resident_data(entry, attr_type);
        }
        let nr = NonResidentData::parse(&entry[offset..]).ok()?;
        let runs_start = offset + nr.data_runs_offset as usize;
        if runs_start >= entry.len() {
            return None;
        }
        let runs = parse_data_runs(&entry[runs_start..]);
        let mut result = Vec::new();
        let cluster_size = self.bpb.cluster_size() as usize;
        for run in &runs {
            if run.cluster == 0 {
                // Sparse: fill with zeros
                let zeros = vec![0u8; run.length as usize * cluster_size];
                result.extend_from_slice(&zeros);
            } else {
                let byte_offset = self.bpb.cluster_to_offset(run.cluster);
                let byte_len = run.length as usize * cluster_size;
                let mut buf = vec![0u8; byte_len];
                if self.read_bytes(byte_offset, &mut buf).is_ok() {
                    result.extend_from_slice(&buf);
                }
            }
        }
        // Trim to real size
        let real_size = nr.real_size as usize;
        if result.len() > real_size {
            result.truncate(real_size);
        }
        Some(result)
    }

    /// List entries in a directory given by MFT index.
    pub fn list_directory(&self, dir_mft_index: u64) -> Result<Vec<DirEntry>, &'static str> {
        let entry = self.read_mft_entry(dir_mft_index)?;
        DIR_LOOKUPS.fetch_add(1, Ordering::Relaxed);
        let mut entries = Vec::new();

        // Parse $INDEX_ROOT attribute
        if let Some(data) = self.read_resident_data(&entry, ATTR_INDEX_ROOT) {
            if data.len() >= 32 {
                let entries_offset = read_u32(&data, 16) as usize + 16;
                self.parse_index_entries(&data, entries_offset, &mut entries);
            }
        }

        // Parse $INDEX_ALLOCATION (non-resident index blocks) if present
        if let Some(alloc_data) = self.read_nonresident_data(&entry, ATTR_INDEX_ALLOCATION) {
            let index_size = self.bpb.index_record_size as usize;
            if index_size > 0 {
                let mut pos = 0;
                while pos + index_size <= alloc_data.len() {
                    let block = &alloc_data[pos..pos + index_size];
                    // Check INDX signature
                    if block.len() >= 28 && read_u32(block, 0) == 0x5844_4E49 {
                        let entries_off = read_u32(block, 24) as usize + 24;
                        self.parse_index_entries(block, entries_off, &mut entries);
                    }
                    pos += index_size;
                }
            }
        }

        Ok(entries)
    }

    /// Parse index entries from a buffer.
    fn parse_index_entries(&self, data: &[u8], start: usize, out: &mut Vec<DirEntry>) {
        let mut pos = start;
        while pos + 16 <= data.len() {
            if let Ok(ie) = IndexEntry::parse(&data[pos..]) {
                if ie.is_last() {
                    break;
                }
                if let Some(ref fname) = ie.file_name {
                    // Skip DOS name entries (namespace 2)
                    if fname.namespace != 2 {
                        out.push(DirEntry {
                            name: fname.name.clone(),
                            mft_ref: ie.mft_reference,
                            is_directory: fname.is_directory(),
                            size: fname.real_size,
                        });
                    }
                }
                let advance = ie.entry_length as usize;
                if advance == 0 {
                    break;
                }
                pos += advance;
            } else {
                break;
            }
        }
    }

    /// Resolve a path to an MFT index, starting from root ($Root = 5).
    pub fn resolve_path(&self, path: &str) -> Result<u64, &'static str> {
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Ok(MFT_ENTRY_ROOT);
        }

        let mut current = MFT_ENTRY_ROOT;
        for component in path.split('/') {
            if component.is_empty() {
                continue;
            }
            let entries = self.list_directory(current)?;
            let found = entries.iter().find(|e| {
                e.name.eq_ignore_ascii_case(component)
            });
            match found {
                Some(entry) => current = entry.mft_ref,
                None => return Err("file not found"),
            }
        }
        Ok(current)
    }

    /// Read a file's $DATA attribute by path.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, &'static str> {
        let mft_index = self.resolve_path(path)?;
        let entry = self.read_mft_entry(mft_index)?;
        let header = MftEntryHeader::parse(&entry)?;
        if header.is_directory() {
            return Err("is a directory");
        }
        // Try non-resident first, then resident
        if let Some(data) = self.read_nonresident_data(&entry, ATTR_DATA) {
            return Ok(data);
        }
        if let Some(data) = self.read_resident_data(&entry, ATTR_DATA) {
            return Ok(data);
        }
        Err("no $DATA attribute found")
    }
}

/// A directory listing entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub mft_ref: u64,
    pub is_directory: bool,
    pub size: u64,
}

// --- Global State ---
static MOUNTED: Mutex<Option<NtfsVolume>> = Mutex::new(None);
static INITIALIZED: spin::Once = spin::Once::new();

/// Initialize the NTFS subsystem.
pub fn init() {
    INITIALIZED.call_once(|| {
        crate::serial_println!("[ntfs] NTFS read-only driver initialized");
    });
}

/// Mount an NTFS volume from raw bytes.
pub fn mount(device_data: Vec<u8>) -> Result<(), &'static str> {
    if device_data.len() < 512 {
        return Err("device too small for NTFS");
    }
    let bpb = NtfsBpb::parse(&device_data)?;
    crate::serial_println!(
        "[ntfs] Volume: {} bytes/sector, {} sectors/cluster, MFT at cluster {}",
        bpb.bytes_per_sector, bpb.sectors_per_cluster, bpb.mft_cluster
    );

    // Try to read volume label from $Volume MFT entry
    let vol = NtfsVolume {
        bpb,
        volume_label: String::from("NTFS"),
        data: device_data,
    };
    let label = if let Ok(entry) = vol.read_mft_entry(MFT_ENTRY_VOLUME) {
        if let Some(data) = vol.read_resident_data(&entry, ATTR_FILE_NAME) {
            if data.len() >= 66 {
                let name_len = data[64] as usize;
                decode_utf16le(&data, 66, name_len)
            } else {
                String::from("NTFS")
            }
        } else {
            String::from("NTFS")
        }
    } else {
        String::from("NTFS")
    };

    let mut mounted = MOUNTED.lock();
    *mounted = Some(NtfsVolume {
        bpb: vol.bpb,
        volume_label: label,
        data: vol.data,
    });
    crate::serial_println!("[ntfs] Volume mounted successfully");
    Ok(())
}

/// List files in a directory path.
pub fn ls(path: &str) -> Result<Vec<DirEntry>, &'static str> {
    let mounted = MOUNTED.lock();
    let vol = mounted.as_ref().ok_or("no NTFS volume mounted")?;
    let mft_index = vol.resolve_path(path)?;
    vol.list_directory(mft_index)
}

/// Read a file by path.
pub fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let mounted = MOUNTED.lock();
    let vol = mounted.as_ref().ok_or("no NTFS volume mounted")?;
    vol.read_file(path)
}

/// Display NTFS volume information.
pub fn ntfs_info() -> String {
    let mounted = MOUNTED.lock();
    match mounted.as_ref() {
        Some(vol) => {
            let total_bytes = vol.bpb.total_sectors * vol.bpb.bytes_per_sector as u64;
            let total_mb = total_bytes / (1024 * 1024);
            format!(
                "NTFS Volume Information:\n\
                 Label:               {}\n\
                 Serial:              {:#018X}\n\
                 Bytes/Sector:        {}\n\
                 Sectors/Cluster:     {}\n\
                 Cluster Size:        {} bytes\n\
                 Total Sectors:       {}\n\
                 Total Size:          {} MB\n\
                 MFT Cluster:         {}\n\
                 MFT Mirror Cluster:  {}\n\
                 MFT Record Size:     {} bytes\n\
                 Index Record Size:   {} bytes",
                vol.volume_label,
                vol.bpb.serial_number,
                vol.bpb.bytes_per_sector,
                vol.bpb.sectors_per_cluster,
                vol.bpb.cluster_size(),
                vol.bpb.total_sectors,
                total_mb,
                vol.bpb.mft_cluster,
                vol.bpb.mft_mirror_cluster,
                vol.bpb.mft_record_size,
                vol.bpb.index_record_size,
            )
        }
        None => String::from("No NTFS volume mounted"),
    }
}

/// Display NTFS statistics.
pub fn ntfs_stats() -> String {
    format!(
        "NTFS Statistics:\n\
         MFT reads:      {}\n\
         Data reads:      {}\n\
         Dir lookups:     {}\n\
         Bytes read:      {}",
        MFT_READS.load(Ordering::Relaxed),
        DATA_READS.load(Ordering::Relaxed),
        DIR_LOOKUPS.load(Ordering::Relaxed),
        BYTES_READ.load(Ordering::Relaxed),
    )
}

/// Special MFT entry description.
pub fn special_entry_name(index: u64) -> &'static str {
    match index {
        0 => "$MFT (Master File Table)",
        1 => "$MFTMirr (MFT Mirror)",
        2 => "$LogFile (Journal)",
        3 => "$Volume (Volume metadata)",
        4 => "$AttrDef (Attribute definitions)",
        5 => "$Root (Root directory)",
        6 => "$Bitmap (Cluster bitmap)",
        7 => "$Boot (Boot sector)",
        8 => "$BadClus (Bad clusters)",
        9 => "$Secure (Security descriptors)",
        10 => "$UpCase (Uppercase table)",
        11 => "$Extend (Extended metadata)",
        _ => "(user file)",
    }
}

// --- Helper functions ---
fn read_u16(data: &[u8], offset: usize) -> u16 {
    if offset + 2 > data.len() { return 0; }
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() { return 0; }
    u32::from_le_bytes([
        data[offset], data[offset + 1],
        data[offset + 2], data[offset + 3],
    ])
}

fn read_u48(data: &[u8], offset: usize) -> u64 {
    if offset + 6 > data.len() { return 0; }
    data[offset] as u64
        | (data[offset + 1] as u64) << 8
        | (data[offset + 2] as u64) << 16
        | (data[offset + 3] as u64) << 24
        | (data[offset + 4] as u64) << 32
        | (data[offset + 5] as u64) << 40
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    if offset + 8 > data.len() { return 0; }
    u64::from_le_bytes([
        data[offset], data[offset + 1],
        data[offset + 2], data[offset + 3],
        data[offset + 4], data[offset + 5],
        data[offset + 6], data[offset + 7],
    ])
}

/// Decode UTF-16LE to a Rust String (lossy conversion).
fn decode_utf16le(data: &[u8], offset: usize, char_count: usize) -> String {
    let mut chars = Vec::with_capacity(char_count);
    for i in 0..char_count {
        let pos = offset + i * 2;
        if pos + 2 <= data.len() {
            let code_unit = u16::from_le_bytes([data[pos], data[pos + 1]]);
            if let Some(c) = core::char::from_u32(code_unit as u32) {
                chars.push(c);
            } else {
                chars.push('\u{FFFD}');
            }
        }
    }
    chars.into_iter().collect()
}
