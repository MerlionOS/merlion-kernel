/// Simple key-value database for MerlionOS.
///
/// Provides sorted key-value storage backed by a `Vec<(String, Vec<u8>)>`
/// kept in lexicographic order by key.  Supports prefix scans, binary
/// serialisation for disk persistence via `crate::diskfs`, and built-in
/// database names for OS subsystems ("system", "users").

use alloc::string::String;
use alloc::vec::Vec;

/// Magic header written at the start of every serialised database image.
const MAGIC: &[u8; 4] = b"KVDB";

/// Wire format version.  Bump when the layout changes.
const VERSION: u8 = 1;

/// A sorted key-value store.
///
/// Entries are kept in a `Vec` sorted by key so that prefix scans and
/// ordered iteration are efficient without pulling in a full B-tree
/// implementation.
pub struct KvDb {
    /// Database name (used as the diskfs filename).
    name: String,
    /// Sorted entries — invariant: sorted by `.0` at all times.
    entries: Vec<(String, Vec<u8>)>,
}

impl KvDb {
    /// Create a new, empty database with the given name.
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            entries: Vec::new(),
        }
    }

    /// Return the database name.
    pub fn name(&self) -> &str {
        &self.name
    }

    // ------------------------------------------------------------------
    // Lookup helpers
    // ------------------------------------------------------------------

    /// Binary-search for the position of `key`.
    /// Returns `Ok(idx)` if found, `Err(idx)` for the insertion point.
    fn pos(&self, key: &str) -> Result<usize, usize> {
        self.entries.binary_search_by(|(k, _)| k.as_str().cmp(key))
    }

    // ------------------------------------------------------------------
    // Core CRUD
    // ------------------------------------------------------------------

    /// Look up a value by key.
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.pos(key).ok().map(|i| self.entries[i].1.as_slice())
    }

    /// Insert or update a key-value pair.
    ///
    /// The entry list is kept sorted; an existing key is updated in place,
    /// otherwise the pair is inserted at the correct position.
    pub fn set(&mut self, key: &str, value: &[u8]) {
        match self.pos(key) {
            Ok(i) => {
                self.entries[i].1 = Vec::from(value);
            }
            Err(i) => {
                self.entries.insert(i, (String::from(key), Vec::from(value)));
            }
        }
    }

    /// Delete a key.  Returns `true` if the key existed and was removed.
    pub fn delete(&mut self, key: &str) -> bool {
        match self.pos(key) {
            Ok(i) => {
                self.entries.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    // ------------------------------------------------------------------
    // Iteration / scan
    // ------------------------------------------------------------------

    /// Return all keys in sorted order.
    pub fn keys(&self) -> Vec<&str> {
        self.entries.iter().map(|(k, _)| k.as_str()).collect()
    }

    /// Return all entries whose key starts with `prefix`, in sorted order.
    pub fn scan(&self, prefix: &str) -> Vec<(&str, &[u8])> {
        // Find the first key >= prefix via binary search.
        let start = match self.pos(prefix) {
            Ok(i) => i,
            Err(i) => i,
        };
        self.entries[start..]
            .iter()
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect()
    }

    // ------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------

    /// Return `(key_count, total_bytes)` where total_bytes covers both keys
    /// and values stored in the database.
    pub fn stats(&self) -> (usize, usize) {
        let total: usize = self
            .entries
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();
        (self.entries.len(), total)
    }

    // ------------------------------------------------------------------
    // Serialisation — compact binary format
    // ------------------------------------------------------------------
    //
    // Layout (all integers little-endian):
    //   [4]  magic "KVDB"
    //   [1]  version
    //   [4]  entry count (u32)
    //   per entry:
    //     [2]  key length   (u16)
    //     [4]  value length (u32)
    //     [n]  key bytes (UTF-8)
    //     [m]  value bytes

    /// Serialise the database into a byte vector suitable for disk storage.
    pub fn serialize(&self) -> Vec<u8> {
        let count = self.entries.len() as u32;
        // Pre-calculate total size.
        let mut size = 4 + 1 + 4; // header
        for (k, v) in &self.entries {
            size += 2 + 4 + k.len() + v.len();
        }
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);
        buf.extend_from_slice(&count.to_le_bytes());

        for (k, v) in &self.entries {
            let klen = k.len() as u16;
            let vlen = v.len() as u32;
            buf.extend_from_slice(&klen.to_le_bytes());
            buf.extend_from_slice(&vlen.to_le_bytes());
            buf.extend_from_slice(k.as_bytes());
            buf.extend_from_slice(v);
        }
        buf
    }

    /// Deserialise a database from bytes previously produced by [`serialize`].
    ///
    /// Returns `None` if the data is malformed or the magic/version doesn't
    /// match.
    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 9 {
            return None;
        }
        if &data[0..4] != MAGIC {
            return None;
        }
        if data[4] != VERSION {
            return None;
        }
        let count = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;

        let mut entries = Vec::with_capacity(count);
        let mut off = 9usize;

        for _ in 0..count {
            if off + 6 > data.len() {
                return None;
            }
            let klen = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
            let vlen = u32::from_le_bytes([
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
            ]) as usize;
            off += 6;
            if off + klen + vlen > data.len() {
                return None;
            }
            let key = core::str::from_utf8(&data[off..off + klen]).ok()?;
            let val = &data[off + klen..off + klen + vlen];
            entries.push((String::from(key), Vec::from(val)));
            off += klen + vlen;
        }

        // The name is not stored in the image; the caller supplies it via
        // `load_from_disk` or sets it afterwards.
        Some(Self {
            name: String::new(),
            entries,
        })
    }

    // ------------------------------------------------------------------
    // Disk persistence via crate::diskfs
    // ------------------------------------------------------------------

    /// Persist the database to the virtio disk under its name.
    ///
    /// The file is stored as `"kvdb_<name>"` so it does not collide with
    /// ordinary user files on the MF16 filesystem.
    pub fn save_to_disk(&self) -> Result<(), &'static str> {
        let fname = disk_filename(&self.name);
        let blob = self.serialize();
        crate::diskfs::write_file(&fname, &blob)
    }

    /// Load a database from the virtio disk.
    ///
    /// Returns `None` if the file does not exist or cannot be deserialised.
    pub fn load_from_disk(name: &str) -> Option<Self> {
        let fname = disk_filename(name);
        let blob = crate::diskfs::read_file(&fname).ok()?;
        let mut db = Self::deserialize(&blob)?;
        db.name = String::from(name);
        Some(db)
    }
}

/// Build the diskfs filename for a database.
fn disk_filename(name: &str) -> String {
    let mut s = String::from("kvdb_");
    s.push_str(name);
    s
}

// ----------------------------------------------------------------------
// Built-in databases
// ----------------------------------------------------------------------

/// Open (or create) the **system** database used for OS configuration
/// (e.g. hostname, timezone, boot flags).
pub fn system_db() -> KvDb {
    KvDb::load_from_disk("system").unwrap_or_else(|| KvDb::new("system"))
}

/// Open (or create) the **users** database used for user account data
/// (e.g. password hashes, home directories, shell preferences).
pub fn users_db() -> KvDb {
    KvDb::load_from_disk("users").unwrap_or_else(|| KvDb::new("users"))
}
