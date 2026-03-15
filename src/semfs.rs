/// Semantic Filesystem — tag-based file organization.
/// Files can be tagged with keywords for semantic search.
/// Extends the VFS with metadata beyond just path names.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

const MAX_TAGS: usize = 128;

static TAGS: Mutex<Vec<FileTag>> = Mutex::new(Vec::new());

struct FileTag {
    path: String,
    tags: Vec<String>,
}

/// Add tags to a file path.
pub fn tag(path: &str, new_tags: &[&str]) {
    let mut store = TAGS.lock();

    // Find existing entry
    for entry in store.iter_mut() {
        if entry.path == path {
            for t in new_tags {
                let tag = t.to_lowercase();
                if !entry.tags.contains(&tag) {
                    entry.tags.push(tag);
                }
            }
            return;
        }
    }

    // Create new entry
    if store.len() < MAX_TAGS {
        store.push(FileTag {
            path: path.to_owned(),
            tags: new_tags.iter().map(|t| t.to_lowercase()).collect(),
        });
    }
}

/// Remove a tag from a file.
pub fn untag(path: &str, tag_name: &str) {
    let mut store = TAGS.lock();
    let tag_lower = tag_name.to_lowercase();
    for entry in store.iter_mut() {
        if entry.path == path {
            entry.tags.retain(|t| t != &tag_lower);
            return;
        }
    }
}

/// Get tags for a file.
pub fn get_tags(path: &str) -> Vec<String> {
    let store = TAGS.lock();
    store.iter()
        .find(|e| e.path == path)
        .map(|e| e.tags.clone())
        .unwrap_or_default()
}

/// Search for files by tag. Returns matching paths.
pub fn search(query: &str) -> Vec<String> {
    let store = TAGS.lock();
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    store.iter()
        .filter(|entry| {
            query_words.iter().any(|q| {
                entry.tags.iter().any(|t| t.contains(q))
                    || entry.path.to_lowercase().contains(q)
            })
        })
        .map(|e| e.path.clone())
        .collect()
}

/// List all tagged files with their tags.
pub fn list_all() -> Vec<(String, Vec<String>)> {
    let store = TAGS.lock();
    store.iter()
        .filter(|e| !e.tags.is_empty())
        .map(|e| (e.path.clone(), e.tags.clone()))
        .collect()
}

/// Auto-tag built-in VFS paths.
pub fn init() {
    tag("/dev/null", &["device", "null", "discard"]);
    tag("/dev/serial", &["device", "serial", "uart", "io"]);
    tag("/proc/uptime", &["system", "time", "uptime", "status"]);
    tag("/proc/meminfo", &["system", "memory", "ram", "status"]);
    tag("/proc/tasks", &["system", "process", "task", "status"]);
}
