/// Access Control Lists (ACLs) for MerlionOS.
/// Extends Unix rwx permissions with per-user and per-group fine-grained
/// access control entries, similar to POSIX ACLs.
///
/// Each file/directory can have an ACL consisting of multiple entries that
/// grant or restrict access for specific users and groups beyond the
/// traditional owner/group/other model. Default ACLs on directories are
/// inherited by newly created files and subdirectories.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum ACL entries per path.
const MAX_ENTRIES_PER_ACL: usize = 32;

/// Maximum number of ACL-bearing paths we track.
const MAX_ACL_PATHS: usize = 512;

/// Maximum number of audit log entries.
const MAX_AUDIT_LOG: usize = 256;

// ── Permission bits ──────────────────────────────────────────────────

/// Permission flags for ACL entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AclPerm {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl AclPerm {
    pub const fn none() -> Self {
        Self { read: false, write: false, execute: false }
    }

    pub const fn all() -> Self {
        Self { read: true, write: true, execute: true }
    }

    pub const fn read_only() -> Self {
        Self { read: true, write: false, execute: false }
    }

    pub const fn read_exec() -> Self {
        Self { read: true, write: false, execute: true }
    }

    pub const fn read_write() -> Self {
        Self { read: true, write: true, execute: false }
    }

    /// Check if `self` is a subset of (allowed by) `mask`.
    pub fn masked_by(&self, mask: &AclPerm) -> AclPerm {
        AclPerm {
            read: self.read && mask.read,
            write: self.write && mask.write,
            execute: self.execute && mask.execute,
        }
    }

    /// Returns true if all requested permissions are present.
    pub fn contains(&self, requested: &AclPerm) -> bool {
        (!requested.read || self.read)
            && (!requested.write || self.write)
            && (!requested.execute || self.execute)
    }

    /// Format as rwx string.
    pub fn as_str(&self) -> String {
        format!(
            "{}{}{}",
            if self.read { "r" } else { "-" },
            if self.write { "w" } else { "-" },
            if self.execute { "x" } else { "-" },
        )
    }

    /// Parse from "rwx", "r--", "rw-", etc.
    pub fn from_str(s: &str) -> Option<Self> {
        if s.len() < 3 { return None; }
        let bytes = s.as_bytes();
        Some(AclPerm {
            read: bytes[0] == b'r',
            write: bytes[1] == b'w',
            execute: bytes[2] == b'x',
        })
    }
}

// ── ACL Entry types ──────────────────────────────────────────────────

/// Tag identifying what kind of entity this ACL entry applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclTag {
    /// The file owner.
    UserObj,
    /// A specific named user (by uid).
    User(u32),
    /// The file owning group.
    GroupObj,
    /// A specific named group (by gid).
    Group(u32),
    /// The mask entry — limits effective permissions for named users/groups.
    Mask,
    /// Other (everyone else).
    Other,
}

impl AclTag {
    /// Format for display.
    pub fn label(&self) -> String {
        match self {
            AclTag::UserObj => String::from("user::"),
            AclTag::User(uid) => format!("user:{}:", uid),
            AclTag::GroupObj => String::from("group::"),
            AclTag::Group(gid) => format!("group:{}:", gid),
            AclTag::Mask => String::from("mask::"),
            AclTag::Other => String::from("other::"),
        }
    }
}

/// A single ACL entry.
#[derive(Debug, Clone)]
pub struct AclEntry {
    pub tag: AclTag,
    pub perm: AclPerm,
}

impl AclEntry {
    pub fn new(tag: AclTag, perm: AclPerm) -> Self {
        Self { tag, perm }
    }

    /// Format as "tag:qualifier:rwx".
    pub fn format(&self) -> String {
        format!("{}{}", self.tag.label(), self.perm.as_str())
    }
}

// ── ACL (collection of entries for a path) ───────────────────────────

/// The ACL attached to a specific path.
#[derive(Debug, Clone)]
pub struct Acl {
    pub path: String,
    pub entries: Vec<AclEntry>,
    /// Default ACL entries (inherited by children if this is a directory).
    pub default_entries: Vec<AclEntry>,
    /// Owner uid of the file.
    pub owner_uid: u32,
    /// Owning group gid of the file.
    pub owner_gid: u32,
}

impl Acl {
    pub fn new(path: &str, owner_uid: u32, owner_gid: u32) -> Self {
        let mut entries = Vec::new();
        // Minimal POSIX ACL: user_obj, group_obj, other
        entries.push(AclEntry::new(AclTag::UserObj, AclPerm::read_write()));
        entries.push(AclEntry::new(AclTag::GroupObj, AclPerm::read_only()));
        entries.push(AclEntry::new(AclTag::Other, AclPerm::read_only()));

        Self {
            path: String::from(path),
            entries,
            default_entries: Vec::new(),
            owner_uid,
            owner_gid,
        }
    }

    /// Add or update an entry. If a matching tag already exists, update it.
    pub fn set_entry(&mut self, entry: AclEntry) {
        if self.entries.len() >= MAX_ENTRIES_PER_ACL {
            return;
        }
        for existing in self.entries.iter_mut() {
            if existing.tag == entry.tag {
                existing.perm = entry.perm;
                return;
            }
        }
        self.entries.push(entry);
        // Ensure mask exists when we have named user/group entries
        self.ensure_mask();
    }

    /// Add or update a default ACL entry.
    pub fn set_default_entry(&mut self, entry: AclEntry) {
        if self.default_entries.len() >= MAX_ENTRIES_PER_ACL {
            return;
        }
        for existing in self.default_entries.iter_mut() {
            if existing.tag == entry.tag {
                existing.perm = entry.perm;
                return;
            }
        }
        self.default_entries.push(entry);
    }

    /// Remove an entry by tag.
    pub fn remove_entry(&mut self, tag: &AclTag) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| &e.tag != tag);
        self.entries.len() < before
    }

    /// Get the mask entry's permissions, or rwx if no mask.
    pub fn get_mask(&self) -> AclPerm {
        for entry in &self.entries {
            if entry.tag == AclTag::Mask {
                return entry.perm;
            }
        }
        AclPerm::all()
    }

    /// Ensure a mask entry exists if there are named user/group entries.
    fn ensure_mask(&mut self) {
        let has_named = self.entries.iter().any(|e| {
            matches!(e.tag, AclTag::User(_) | AclTag::Group(_))
        });
        if !has_named {
            return;
        }
        let has_mask = self.entries.iter().any(|e| e.tag == AclTag::Mask);
        if !has_mask {
            // Default mask: union of all named + group_obj permissions
            let mut mask = AclPerm::none();
            for entry in &self.entries {
                match entry.tag {
                    AclTag::User(_) | AclTag::Group(_) | AclTag::GroupObj => {
                        mask.read = mask.read || entry.perm.read;
                        mask.write = mask.write || entry.perm.write;
                        mask.execute = mask.execute || entry.perm.execute;
                    }
                    _ => {}
                }
            }
            self.entries.push(AclEntry::new(AclTag::Mask, mask));
        }
    }

    /// Check if the given uid/gid has the requested access.
    pub fn check_access(&self, uid: u32, gid: u32, requested: &AclPerm) -> bool {
        // Step 1: If uid matches owner, use UserObj permissions directly
        if uid == self.owner_uid {
            for entry in &self.entries {
                if entry.tag == AclTag::UserObj {
                    return entry.perm.contains(requested);
                }
            }
            return false;
        }

        let mask = self.get_mask();

        // Step 2: Check named user entries
        for entry in &self.entries {
            if let AclTag::User(entry_uid) = entry.tag {
                if entry_uid == uid {
                    let effective = entry.perm.masked_by(&mask);
                    return effective.contains(requested);
                }
            }
        }

        // Step 3: If gid matches owning group, use GroupObj (masked)
        if gid == self.owner_gid {
            for entry in &self.entries {
                if entry.tag == AclTag::GroupObj {
                    let effective = entry.perm.masked_by(&mask);
                    return effective.contains(requested);
                }
            }
        }

        // Step 4: Check named group entries
        for entry in &self.entries {
            if let AclTag::Group(entry_gid) = entry.tag {
                if entry_gid == gid {
                    let effective = entry.perm.masked_by(&mask);
                    return effective.contains(requested);
                }
            }
        }

        // Step 5: Fall through to Other
        for entry in &self.entries {
            if entry.tag == AclTag::Other {
                return entry.perm.contains(requested);
            }
        }

        false
    }

    /// Format the ACL in getfacl-compatible output.
    pub fn format_getfacl(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# file: {}\n", self.path));
        out.push_str(&format!("# owner: {}\n", self.owner_uid));
        out.push_str(&format!("# group: {}\n", self.owner_gid));

        // Access ACL entries
        for entry in &self.entries {
            let effective = match entry.tag {
                AclTag::User(_) | AclTag::Group(_) | AclTag::GroupObj => {
                    let mask = self.get_mask();
                    let eff = entry.perm.masked_by(&mask);
                    if eff.read != entry.perm.read
                        || eff.write != entry.perm.write
                        || eff.execute != entry.perm.execute
                    {
                        format!("\t#effective:{}", eff.as_str())
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };
            out.push_str(&format!("{}{}\n", entry.format(), effective));
        }

        // Default ACL entries
        if !self.default_entries.is_empty() {
            out.push_str("\n");
            for entry in &self.default_entries {
                out.push_str(&format!("default:{}\n", entry.format()));
            }
        }

        out
    }
}

// ── Audit log ────────────────────────────────────────────────────────

/// Audit decision type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDecision {
    Allow,
    Deny,
}

/// A single audit record.
#[derive(Debug, Clone)]
struct AuditRecord {
    uid: u32,
    gid: u32,
    path: String,
    requested: AclPerm,
    decision: AuditDecision,
    tick: u64,
}

struct AuditLog {
    records: Vec<AuditRecord>,
}

impl AuditLog {
    const fn new() -> Self {
        Self { records: Vec::new() }
    }

    fn log(&mut self, record: AuditRecord) {
        if self.records.len() >= MAX_AUDIT_LOG {
            self.records.remove(0);
        }
        self.records.push(record);
    }

    fn recent(&self, count: usize) -> Vec<&AuditRecord> {
        let start = if self.records.len() > count {
            self.records.len() - count
        } else {
            0
        };
        self.records[start..].iter().collect()
    }

    fn deny_count(&self) -> usize {
        self.records.iter().filter(|r| r.decision == AuditDecision::Deny).count()
    }

    fn allow_count(&self) -> usize {
        self.records.iter().filter(|r| r.decision == AuditDecision::Allow).count()
    }
}

// ── Global ACL table ─────────────────────────────────────────────────

struct AclTable {
    acls: Vec<Acl>,
}

impl AclTable {
    const fn new() -> Self {
        Self { acls: Vec::new() }
    }

    fn find(&self, path: &str) -> Option<&Acl> {
        self.acls.iter().find(|a| a.path == path)
    }

    fn find_mut(&mut self, path: &str) -> Option<&mut Acl> {
        self.acls.iter_mut().find(|a| a.path == path)
    }

    fn get_or_create(&mut self, path: &str, owner_uid: u32, owner_gid: u32) -> &mut Acl {
        if self.find(path).is_none() {
            if self.acls.len() >= MAX_ACL_PATHS {
                // Evict oldest
                self.acls.remove(0);
            }
            self.acls.push(Acl::new(path, owner_uid, owner_gid));
        }
        self.find_mut(path).unwrap()
    }

    fn remove(&mut self, path: &str) -> bool {
        let before = self.acls.len();
        self.acls.retain(|a| a.path != path);
        self.acls.len() < before
    }

    fn count(&self) -> usize {
        self.acls.len()
    }

    fn total_entries(&self) -> usize {
        self.acls.iter().map(|a| a.entries.len() + a.default_entries.len()).sum()
    }
}

// ── Global state ─────────────────────────────────────────────────────

static ACL_TABLE: Mutex<AclTable> = Mutex::new(AclTable::new());
static AUDIT: Mutex<AuditLog> = Mutex::new(AuditLog::new());

static CHECKS_TOTAL: AtomicU64 = AtomicU64::new(0);
static CHECKS_ALLOWED: AtomicU64 = AtomicU64::new(0);
static CHECKS_DENIED: AtomicU64 = AtomicU64::new(0);
static SETS_TOTAL: AtomicU64 = AtomicU64::new(0);
static REMOVES_TOTAL: AtomicU64 = AtomicU64::new(0);

// ── Public API ───────────────────────────────────────────────────────

/// Initialize the ACL subsystem with some default ACLs.
pub fn init() {
    let mut table = ACL_TABLE.lock();

    // Set up default ACLs for key system paths
    let root_acl = table.get_or_create("/", 0, 0);
    root_acl.set_entry(AclEntry::new(AclTag::UserObj, AclPerm::all()));
    root_acl.set_entry(AclEntry::new(AclTag::GroupObj, AclPerm::read_exec()));
    root_acl.set_entry(AclEntry::new(AclTag::Other, AclPerm::read_exec()));

    let etc_acl = table.get_or_create("/etc", 0, 0);
    etc_acl.set_entry(AclEntry::new(AclTag::UserObj, AclPerm::all()));
    etc_acl.set_entry(AclEntry::new(AclTag::GroupObj, AclPerm::read_exec()));
    etc_acl.set_entry(AclEntry::new(AclTag::Other, AclPerm::read_only()));

    let home_acl = table.get_or_create("/home", 0, 0);
    home_acl.set_entry(AclEntry::new(AclTag::UserObj, AclPerm::all()));
    home_acl.set_entry(AclEntry::new(AclTag::GroupObj, AclPerm::read_exec()));
    home_acl.set_entry(AclEntry::new(AclTag::Other, AclPerm::read_exec()));
    // Default ACL for new items in /home
    home_acl.set_default_entry(AclEntry::new(AclTag::UserObj, AclPerm::all()));
    home_acl.set_default_entry(AclEntry::new(AclTag::GroupObj, AclPerm::read_exec()));
    home_acl.set_default_entry(AclEntry::new(AclTag::Other, AclPerm::none()));

    let tmp_acl = table.get_or_create("/tmp", 0, 0);
    tmp_acl.set_entry(AclEntry::new(AclTag::UserObj, AclPerm::all()));
    tmp_acl.set_entry(AclEntry::new(AclTag::GroupObj, AclPerm::all()));
    tmp_acl.set_entry(AclEntry::new(AclTag::Other, AclPerm::all()));

    let dev_acl = table.get_or_create("/dev", 0, 0);
    dev_acl.set_entry(AclEntry::new(AclTag::UserObj, AclPerm::all()));
    dev_acl.set_entry(AclEntry::new(AclTag::GroupObj, AclPerm::read_write()));
    dev_acl.set_entry(AclEntry::new(AclTag::Other, AclPerm::read_only()));

    let proc_acl = table.get_or_create("/proc", 0, 0);
    proc_acl.set_entry(AclEntry::new(AclTag::UserObj, AclPerm::read_only()));
    proc_acl.set_entry(AclEntry::new(AclTag::GroupObj, AclPerm::read_only()));
    proc_acl.set_entry(AclEntry::new(AclTag::Other, AclPerm::read_only()));

    drop(table);
    crate::serial_println!("[ok] ACL subsystem initialized");
}

/// Set an ACL entry on a path. `spec` format: "u:1000:rwx" or "g:100:r-x" or "m::rw-" etc.
pub fn setacl(path: &str, spec: &str) -> Result<(), String> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() < 3 {
        return Err(String::from("Invalid ACL spec. Format: u:uid:rwx / g:gid:rwx / m::rwx / o::rwx"));
    }

    let tag = match parts[0] {
        "u" | "user" => {
            if parts[1].is_empty() {
                AclTag::UserObj
            } else {
                let uid = parts[1].parse::<u32>().map_err(|_| String::from("Invalid uid"))?;
                AclTag::User(uid)
            }
        }
        "g" | "group" => {
            if parts[1].is_empty() {
                AclTag::GroupObj
            } else {
                let gid = parts[1].parse::<u32>().map_err(|_| String::from("Invalid gid"))?;
                AclTag::Group(gid)
            }
        }
        "m" | "mask" => AclTag::Mask,
        "o" | "other" => AclTag::Other,
        _ => return Err(format!("Unknown tag type: {}", parts[0])),
    };

    let perm = AclPerm::from_str(parts[2])
        .ok_or_else(|| String::from("Invalid permission string. Use rwx format."))?;

    let mut table = ACL_TABLE.lock();
    let acl = table.get_or_create(path, 0, 0);
    acl.set_entry(AclEntry::new(tag, perm));
    SETS_TOTAL.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

/// Set a default ACL entry on a directory path.
pub fn set_default_acl(path: &str, spec: &str) -> Result<(), String> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() < 3 {
        return Err(String::from("Invalid ACL spec. Format: u:uid:rwx"));
    }

    let tag = match parts[0] {
        "u" | "user" => {
            if parts[1].is_empty() {
                AclTag::UserObj
            } else {
                let uid = parts[1].parse::<u32>().map_err(|_| String::from("Invalid uid"))?;
                AclTag::User(uid)
            }
        }
        "g" | "group" => {
            if parts[1].is_empty() {
                AclTag::GroupObj
            } else {
                let gid = parts[1].parse::<u32>().map_err(|_| String::from("Invalid gid"))?;
                AclTag::Group(gid)
            }
        }
        "m" | "mask" => AclTag::Mask,
        "o" | "other" => AclTag::Other,
        _ => return Err(format!("Unknown tag type: {}", parts[0])),
    };

    let perm = AclPerm::from_str(parts[2])
        .ok_or_else(|| String::from("Invalid permission string."))?;

    let mut table = ACL_TABLE.lock();
    let acl = table.get_or_create(path, 0, 0);
    acl.set_default_entry(AclEntry::new(tag, perm));
    SETS_TOTAL.fetch_add(1, Ordering::Relaxed);

    Ok(())
}

/// Get ACL for a path in getfacl format.
pub fn getacl(path: &str) -> String {
    let table = ACL_TABLE.lock();
    match table.find(path) {
        Some(acl) => acl.format_getfacl(),
        None => format!("No ACL set for {}", path),
    }
}

/// Remove all ACL entries for a path.
pub fn removeacl(path: &str) -> bool {
    let result = ACL_TABLE.lock().remove(path);
    if result {
        REMOVES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Check ACL access — the main integration point.
/// Returns true if access is allowed, false otherwise.
/// Logs the decision to the audit trail.
pub fn acl_check(uid: u32, gid: u32, path: &str, requested: &AclPerm) -> bool {
    CHECKS_TOTAL.fetch_add(1, Ordering::Relaxed);

    let table = ACL_TABLE.lock();
    let decision = match table.find(path) {
        Some(acl) => {
            if acl.check_access(uid, gid, requested) {
                AuditDecision::Allow
            } else {
                AuditDecision::Deny
            }
        }
        None => {
            // No ACL set — default allow (fall back to standard perms)
            AuditDecision::Allow
        }
    };
    drop(table);

    match decision {
        AuditDecision::Allow => CHECKS_ALLOWED.fetch_add(1, Ordering::Relaxed),
        AuditDecision::Deny => CHECKS_DENIED.fetch_add(1, Ordering::Relaxed),
    };

    let record = AuditRecord {
        uid,
        gid,
        path: String::from(path),
        requested: *requested,
        decision,
        tick: crate::timer::ticks(),
    };
    AUDIT.lock().log(record);

    decision == AuditDecision::Allow
}

/// Format ACL for a path (alias for getacl).
pub fn format_acl(path: &str) -> String {
    getacl(path)
}

/// Get recent audit log entries.
pub fn audit_log(count: usize) -> String {
    let audit = AUDIT.lock();
    let records = audit.recent(count);
    if records.is_empty() {
        return String::from("No ACL audit records.");
    }

    let mut out = String::from("ACL Audit Log (recent):\n");
    out.push_str("  TICK       UID  GID  DECISION  REQUESTED  PATH\n");
    out.push_str("  ─────────  ───  ───  ────────  ─────────  ────\n");
    for rec in &records {
        let dec_str = match rec.decision {
            AuditDecision::Allow => "ALLOW   ",
            AuditDecision::Deny => "DENY    ",
        };
        out.push_str(&format!(
            "  {:>9}  {:>3}  {:>3}  {}  {}      {}\n",
            rec.tick, rec.uid, rec.gid, dec_str, rec.requested.as_str(), rec.path,
        ));
    }
    out
}

/// Overall ACL subsystem info.
pub fn acl_info() -> String {
    let table = ACL_TABLE.lock();
    let path_count = table.count();
    let entry_count = table.total_entries();

    let mut out = String::from("=== ACL Subsystem ===\n");
    out.push_str(&format!("Paths with ACLs : {}\n", path_count));
    out.push_str(&format!("Total entries    : {}\n", entry_count));
    out.push_str(&format!("Max paths        : {}\n", MAX_ACL_PATHS));
    out.push_str(&format!("Max entries/path : {}\n", MAX_ENTRIES_PER_ACL));
    out.push_str("\nACL-bearing paths:\n");
    for acl in &table.acls {
        out.push_str(&format!(
            "  {} (owner={}:{}, {} entries, {} defaults)\n",
            acl.path, acl.owner_uid, acl.owner_gid,
            acl.entries.len(), acl.default_entries.len(),
        ));
    }
    out
}

/// ACL statistics.
pub fn acl_stats() -> String {
    let checks = CHECKS_TOTAL.load(Ordering::Relaxed);
    let allowed = CHECKS_ALLOWED.load(Ordering::Relaxed);
    let denied = CHECKS_DENIED.load(Ordering::Relaxed);
    let sets = SETS_TOTAL.load(Ordering::Relaxed);
    let removes = REMOVES_TOTAL.load(Ordering::Relaxed);

    let audit = AUDIT.lock();
    let audit_total = audit.records.len();
    let audit_denies = audit.deny_count();
    let audit_allows = audit.allow_count();
    drop(audit);

    let table = ACL_TABLE.lock();
    let paths = table.count();
    let entries = table.total_entries();
    drop(table);

    let mut out = String::from("=== ACL Statistics ===\n");
    out.push_str(&format!("Access checks    : {}\n", checks));
    out.push_str(&format!("  Allowed        : {}\n", allowed));
    out.push_str(&format!("  Denied         : {}\n", denied));
    out.push_str(&format!("ACL set ops      : {}\n", sets));
    out.push_str(&format!("ACL remove ops   : {}\n", removes));
    out.push_str(&format!("Active paths     : {}\n", paths));
    out.push_str(&format!("Total entries    : {}\n", entries));
    out.push_str(&format!("Audit records    : {}\n", audit_total));
    out.push_str(&format!("  Audit allows   : {}\n", audit_allows));
    out.push_str(&format!("  Audit denies   : {}\n", audit_denies));
    out
}

/// List all ACLs in summary form.
pub fn list_acls() -> String {
    let table = ACL_TABLE.lock();
    if table.acls.is_empty() {
        return String::from("No ACLs configured.");
    }

    let mut out = String::from("Configured ACLs:\n");
    for acl in &table.acls {
        out.push_str(&format!("\n{}", acl.format_getfacl()));
    }
    out
}

/// Inherit default ACLs from parent directory when creating a new file/dir.
pub fn inherit_acl(parent_path: &str, child_path: &str, owner_uid: u32, owner_gid: u32) {
    let table_guard = ACL_TABLE.lock();
    let defaults = match table_guard.find(parent_path) {
        Some(parent_acl) if !parent_acl.default_entries.is_empty() => {
            parent_acl.default_entries.clone()
        }
        _ => return,
    };
    drop(table_guard);

    let mut table = ACL_TABLE.lock();
    let child_acl = table.get_or_create(child_path, owner_uid, owner_gid);
    for entry in defaults {
        child_acl.set_entry(entry);
    }
}

/// Parse a setacl command line: "setacl /path u:1000:rwx"
pub fn parse_setacl_cmd(args: &str) -> Result<(), String> {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 {
        return Err(String::from("Usage: setacl <path> <u|g|m|o>:[id]:rwx"));
    }
    let path = parts[0].trim();
    let spec = parts[1].trim();

    if spec.starts_with("d:") || spec.starts_with("default:") {
        let actual_spec = if spec.starts_with("d:") {
            &spec[2..]
        } else {
            &spec[8..]
        };
        set_default_acl(path, actual_spec)
    } else {
        setacl(path, spec)
    }
}
