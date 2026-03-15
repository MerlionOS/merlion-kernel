/// Security and permissions module for MerlionOS.
/// Provides Unix-style rwxrwxrwx file permissions, user/group management,
/// authentication with FNV-1a password hashing, and user switching.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

const MAX_USERS: usize = 64;
const MAX_GROUPS: usize = 64;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

/// Unix-style file permission bits (rwxrwxrwx).
/// Stored as a `u16` where the lower 9 bits encode
/// owner (bits 8-6), group (bits 5-3), and other (bits 2-0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permission {
    /// Raw permission bits (lower 9 = rwxrwxrwx).
    pub bits: u16,
    /// UID of the file owner.
    pub owner_uid: u32,
    /// GID of the owning group.
    pub owner_gid: u32,
}

impl Permission {
    pub const READ: u16 = 0b100;
    pub const WRITE: u16 = 0b010;
    pub const EXEC: u16 = 0b001;

    /// Create a permission from an octal mode (e.g. 0o755).
    pub fn new(mode: u16, owner_uid: u32, owner_gid: u32) -> Self {
        Self { bits: mode & 0o777, owner_uid, owner_gid }
    }

    /// Default directory permission: rwxr-xr-x owned by root:root.
    pub fn default_dir() -> Self { Self::new(0o755, 0, 0) }

    /// Default file permission: rw-r--r-- owned by root:root.
    pub fn default_file() -> Self { Self::new(0o644, 0, 0) }

    /// Return the 3-bit owner field.
    pub fn owner(&self) -> u16 { (self.bits >> 6) & 0o7 }
    /// Return the 3-bit group field.
    pub fn group(&self) -> u16 { (self.bits >> 3) & 0o7 }
    /// Return the 3-bit other field.
    pub fn other(&self) -> u16 { self.bits & 0o7 }

    /// Format as a "rwxrwxrwx" string.
    pub fn display(&self) -> String {
        let mut s = String::with_capacity(9);
        for shift in (0..3).rev() {
            let t = (self.bits >> (shift * 3)) & 0o7;
            s.push(if t & 0b100 != 0 { 'r' } else { '-' });
            s.push(if t & 0b010 != 0 { 'w' } else { '-' });
            s.push(if t & 0b001 != 0 { 'x' } else { '-' });
        }
        s
    }
}

/// A system user.
#[derive(Debug, Clone)]
pub struct User {
    pub uid: u32,
    pub name: String,
    pub password_hash: u64,
    pub groups: Vec<u32>,
}

/// A system group.
#[derive(Debug, Clone)]
pub struct Group {
    pub gid: u32,
    pub name: String,
    pub members: Vec<u32>,
}

/// In-kernel user/group database.
struct UserDb {
    users: Vec<User>,
    groups: Vec<Group>,
    current_uid: u32,
}

static USER_DB: Mutex<Option<UserDb>> = Mutex::new(None);

/// Side table storing (path, Permission) pairs for VFS files.
static PERM_TABLE: Mutex<Vec<(String, Permission)>> = Mutex::new(Vec::new());

/// Initialise the user database with default users and groups.
///
/// Default users: root (uid 0), system (uid 1), user (uid 1000).
/// Default groups: root (gid 0), system (gid 1), users (gid 1000).
pub fn init() {
    let mut db = UserDb { users: Vec::new(), groups: Vec::new(), current_uid: 0 };
    let empty_hash = hash_password("");

    db.groups.push(Group { gid: 0, name: "root".to_owned(), members: alloc::vec![0] });
    db.groups.push(Group { gid: 1, name: "system".to_owned(), members: alloc::vec![1] });
    db.groups.push(Group { gid: 1000, name: "users".to_owned(), members: alloc::vec![1000] });

    db.users.push(User {
        uid: 0, name: "root".to_owned(), password_hash: empty_hash, groups: alloc::vec![0],
    });
    db.users.push(User {
        uid: 1, name: "system".to_owned(), password_hash: empty_hash, groups: alloc::vec![1],
    });
    db.users.push(User {
        uid: 1000, name: "user".to_owned(), password_hash: empty_hash, groups: alloc::vec![1000],
    });

    *USER_DB.lock() = Some(db);
}

/// Compute a 64-bit FNV-1a hash of the given password string.
/// Intentionally simple — suitable for a hobby OS, not production use.
pub fn hash_password(password: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in password.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Authenticate a user by name and password hash.
/// Returns `true` if the user exists and the hash matches.
pub fn authenticate(username: &str, password_hash: u64) -> bool {
    let lock = USER_DB.lock();
    match lock.as_ref() {
        Some(db) => db.users.iter().any(|u| u.name == username && u.password_hash == password_hash),
        None => false,
    }
}

/// Check whether `uid` has the requested `access` (3-bit rwx mask) on `perms`.
/// Root (uid 0) always has full access.
pub fn check_permission(uid: u32, perms: &Permission, access: u16) -> bool {
    if uid == 0 { return true; }

    if uid == perms.owner_uid {
        return (perms.owner() & access) == access;
    }

    // Group check
    let lock = USER_DB.lock();
    if let Some(db) = lock.as_ref() {
        if let Some(user) = db.users.iter().find(|u| u.uid == uid) {
            if user.groups.contains(&perms.owner_gid) {
                return (perms.group() & access) == access;
            }
        }
    }

    (perms.other() & access) == access
}

/// Add a new user to the database.
/// Returns `Err` if the database is full or the username/uid already exists.
pub fn add_user(uid: u32, name: &str, password: &str, groups: &[u32]) -> Result<(), &'static str> {
    let mut lock = USER_DB.lock();
    let db = lock.as_mut().ok_or("security: not initialised")?;
    if db.users.len() >= MAX_USERS {
        return Err("security: max users reached");
    }
    if db.users.iter().any(|u| u.name == name || u.uid == uid) {
        return Err("security: user already exists");
    }

    let pw_hash = hash_password(password);
    let group_vec: Vec<u32> = groups.iter().copied().collect();

    for &gid in &group_vec {
        if let Some(g) = db.groups.iter_mut().find(|g| g.gid == gid) {
            if !g.members.contains(&uid) { g.members.push(uid); }
        }
    }

    db.users.push(User { uid, name: name.to_owned(), password_hash: pw_hash, groups: group_vec });
    Ok(())
}

/// Remove a user by UID. Cannot remove root (uid 0).
pub fn remove_user(uid: u32) -> Result<(), &'static str> {
    if uid == 0 { return Err("security: cannot remove root"); }
    let mut lock = USER_DB.lock();
    let db = lock.as_mut().ok_or("security: not initialised")?;
    let idx = db.users.iter().position(|u| u.uid == uid)
        .ok_or("security: user not found")?;
    for g in db.groups.iter_mut() { g.members.retain(|&m| m != uid); }
    db.users.remove(idx);
    Ok(())
}

/// Add a new group to the database.
pub fn add_group(gid: u32, name: &str) -> Result<(), &'static str> {
    let mut lock = USER_DB.lock();
    let db = lock.as_mut().ok_or("security: not initialised")?;
    if db.groups.len() >= MAX_GROUPS {
        return Err("security: max groups reached");
    }
    if db.groups.iter().any(|g| g.name == name || g.gid == gid) {
        return Err("security: group already exists");
    }
    db.groups.push(Group { gid, name: name.to_owned(), members: Vec::new() });
    Ok(())
}

/// Set permission mode on a VFS path (e.g. 0o644).
/// Only root or the file owner may change permissions.
pub fn chmod(path: &str, mode: u16) -> Result<(), &'static str> {
    if !crate::vfs::exists(path) {
        return Err("security: file not found");
    }
    let uid = current_uid();
    let mut lock = PERM_TABLE.lock();
    if let Some(entry) = lock.iter_mut().find(|e| e.0 == path) {
        if uid != 0 && uid != entry.1.owner_uid {
            return Err("security: permission denied");
        }
        entry.1.bits = mode & 0o777;
    } else {
        lock.push((path.to_owned(), Permission::new(mode, uid, 0)));
    }
    Ok(())
}

/// Look up the permission for a VFS path, returning a default if none set.
pub fn get_permission(path: &str) -> Permission {
    let lock = PERM_TABLE.lock();
    lock.iter().find(|e| e.0 == path).map(|e| e.1).unwrap_or_else(Permission::default_file)
}

/// Return the UID of the currently active user.
pub fn current_uid() -> u32 {
    let lock = USER_DB.lock();
    match lock.as_ref() { Some(db) => db.current_uid, None => 0 }
}

/// Return the login name of the currently active user.
pub fn whoami() -> String {
    let lock = USER_DB.lock();
    match lock.as_ref() {
        Some(db) => db.users.iter()
            .find(|u| u.uid == db.current_uid)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "unknown".to_owned()),
        None => "root".to_owned(),
    }
}

/// Switch the active user. Root can switch without a password.
/// Non-root users must supply the correct password for the target account.
pub fn su(username: &str, password: Option<&str>) -> Result<(), &'static str> {
    let mut lock = USER_DB.lock();
    let db = lock.as_mut().ok_or("security: not initialised")?;
    let caller_uid = db.current_uid;
    let target = db.users.iter()
        .find(|u| u.name == username)
        .ok_or("security: user not found")?;
    let target_uid = target.uid;

    if caller_uid != 0 {
        let pw = password.ok_or("security: password required")?;
        if target.password_hash != hash_password(pw) {
            return Err("security: authentication failed");
        }
    }
    db.current_uid = target_uid;
    Ok(())
}

/// List all users as (uid, name) pairs.
pub fn list_users() -> Vec<(u32, String)> {
    let lock = USER_DB.lock();
    match lock.as_ref() {
        Some(db) => db.users.iter().map(|u| (u.uid, u.name.clone())).collect(),
        None => Vec::new(),
    }
}

/// List all groups as (gid, name) pairs.
pub fn list_groups() -> Vec<(u32, String)> {
    let lock = USER_DB.lock();
    match lock.as_ref() {
        Some(db) => db.groups.iter().map(|g| (g.gid, g.name.clone())).collect(),
        None => Vec::new(),
    }
}
