/// Kernel configuration system.
/// Reads key-value settings from /etc/merlion.conf (VFS file).
/// Provides typed access to configuration values.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use spin::Mutex;

static CONFIG: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Load configuration from /etc/merlion.conf.
pub fn load() {
    // Create /etc directory and default config if not exists
    let _ = crate::vfs::write("/etc/merlion.conf",
        "# MerlionOS Configuration\n\
         hostname=merlion\n\
         prompt=merlion> \n\
         heap_warn_pct=70\n\
         heap_crit_pct=90\n\
         max_tasks=8\n\
         ai_proxy=com2\n\
         log_level=info\n"
    );

    // Parse the config file
    if let Ok(content) = crate::vfs::cat("/etc/merlion.conf") {
        let mut config = CONFIG.lock();
        config.clear();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                config.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
        crate::klog_println!("[kconfig] loaded {} settings", config.len());
    }
}

/// Get a string config value.
pub fn get(key: &str) -> Option<String> {
    let config = CONFIG.lock();
    config.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// Get a config value as usize.
pub fn get_usize(key: &str) -> Option<usize> {
    get(key)?.parse().ok()
}

/// Set a config value (runtime only, doesn't persist).
pub fn set(key: &str, value: &str) {
    let mut config = CONFIG.lock();
    for entry in config.iter_mut() {
        if entry.0 == key {
            entry.1 = value.to_owned();
            return;
        }
    }
    config.push((key.to_owned(), value.to_owned()));
}

/// List all config entries.
pub fn list() -> Vec<(String, String)> {
    CONFIG.lock().clone()
}

/// Save current config back to /etc/merlion.conf.
pub fn save() -> Result<(), &'static str> {
    let config = CONFIG.lock();
    let mut content = String::from("# MerlionOS Configuration\n");
    for (key, value) in config.iter() {
        content.push_str(key);
        content.push('=');
        content.push_str(value);
        content.push('\n');
    }
    drop(config);
    crate::vfs::write("/etc/merlion.conf", &content)
}
