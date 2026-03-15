/// Package manager for MerlionOS.
/// Provides package metadata, a registry of installed packages, installation
/// and removal via the VFS, search, verification, and display formatting.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::vfs;
use crate::version;

/// Maximum number of packages the registry can hold.
const MAX_PACKAGES: usize = 64;

/// Base URL for the package repository.
const REPO_BASE_URL: &str = "https://packages.merlionos.dev/v1";

/// FNV-1a offset basis (64-bit).
const FNV_OFFSET: u64 = 0xcbf29ce484222325;

/// FNV-1a prime (64-bit).
const FNV_PRIME: u64 = 0x00000100000001B3;

/// Metadata describing a single package.
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    /// Package name (unique identifier).
    pub name: String,
    /// Semantic version string (e.g. "1.2.3").
    pub version: String,
    /// Short human-readable description.
    pub description: String,
    /// Author or maintainer.
    pub author: String,
    /// Names of packages this one depends on.
    pub dependencies: Vec<String>,
    /// Installed size in bytes.
    pub size_bytes: u64,
}

/// Global registry of installed packages, protected by a spin mutex.
pub struct PackageRegistry {
    installed: Mutex<Vec<PackageMetadata>>,
}

/// The singleton package registry.
static REGISTRY: PackageRegistry = PackageRegistry {
    installed: Mutex::new(Vec::new()),
};

/// Build the manifest download URL for a given package name.
fn manifest_url(name: &str) -> String {
    format!("{}/packages/{}/manifest.toml", REPO_BASE_URL, name)
}

/// Build the full list of dependencies that must be installed for a package.
/// Currently returns a stub list derived from the package name.
fn resolve_dependencies(name: &str) -> Vec<String> {
    // Stub: in a real implementation this would fetch and parse the manifest.
    // For now, return an empty dependency list for every package.
    let _ = name;
    Vec::new()
}

/// Install a package by name.
///
/// Builds the manifest URL, resolves dependencies, and creates the package
/// directory tree inside `/pkg/<name>/` in the VFS.  Returns `Ok(())` on
/// success or an error string on failure.
pub fn install(name: &str) -> Result<(), &'static str> {
    let mut installed = REGISTRY.installed.lock();

    // Check if already installed.
    for pkg in installed.iter() {
        if pkg.name == name {
            return Err("package already installed");
        }
    }

    // Capacity guard.
    if installed.len() >= MAX_PACKAGES {
        return Err("package registry full");
    }

    // Build manifest URL (stub — actual HTTP fetch not performed here).
    let _url = manifest_url(name);

    // Resolve dependency tree.
    let deps = resolve_dependencies(name);

    // Create the package directory in VFS: /pkg/<name>/
    let dir_path = format!("/pkg/{}", name);

    // Ensure /pkg exists.
    if !vfs::exists("/pkg") {
        vfs::write("/pkg/.pkgindex", "MerlionOS package index\n")?;
    }

    // Write a small marker file so the directory entry is visible.
    let marker = format!("{}/PACKAGE", dir_path);
    let info = format!(
        "name={}\nversion=0.1.0\nos={} {}\n",
        name,
        version::NAME,
        version::VERSION
    );
    vfs::write(&marker, &info)?;

    // Record metadata.
    let meta = PackageMetadata {
        name: String::from(name),
        version: String::from("0.1.0"),
        description: format!("{} package for {}", name, version::NAME),
        author: String::from("MerlionOS Community"),
        dependencies: deps,
        size_bytes: info.len() as u64,
    };

    installed.push(meta);
    Ok(())
}

/// Remove an installed package by name.
///
/// Deletes the package marker file from the VFS and removes the metadata
/// entry from the registry.
pub fn remove(name: &str) -> Result<(), &'static str> {
    let mut installed = REGISTRY.installed.lock();

    let pos = installed
        .iter()
        .position(|p| p.name == name)
        .ok_or("package not installed")?;

    // Remove the VFS marker file.
    let marker = format!("/pkg/{}/PACKAGE", name);
    let _ = vfs::rm(&marker); // best-effort

    installed.remove(pos);
    Ok(())
}

/// Return a snapshot of all installed packages.
pub fn list_installed() -> Vec<PackageMetadata> {
    REGISTRY.installed.lock().clone()
}

/// Search installed packages whose name contains the query substring.
pub fn search(query: &str) -> Vec<PackageMetadata> {
    let installed = REGISTRY.installed.lock();
    let q = query.to_ascii_lowercase();
    installed
        .iter()
        .filter(|p| p.name.to_ascii_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Return a human-readable info string for an installed package.
///
/// Includes name, version, author, description, dependencies, and size.
/// Returns `None` if the package is not found.
pub fn pkg_info(name: &str) -> Option<String> {
    let installed = REGISTRY.installed.lock();
    let pkg = installed.iter().find(|p| p.name == name)?;

    let deps = if pkg.dependencies.is_empty() {
        String::from("(none)")
    } else {
        pkg.dependencies.join(", ")
    };

    Some(format!(
        "Name:         {}\n\
         Version:      {}\n\
         Author:       {}\n\
         Description:  {}\n\
         Dependencies: {}\n\
         Size:         {} bytes",
        pkg.name, pkg.version, pkg.author, pkg.description, deps, pkg.size_bytes
    ))
}

/// Verify data integrity using the FNV-1a hash algorithm.
///
/// Computes the 64-bit FNV-1a hash of `data` and compares it to
/// `expected_hash`.  Returns `true` if they match.
pub fn verify_hash(data: &[u8], expected_hash: u64) -> bool {
    let mut hash = FNV_OFFSET;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash == expected_hash
}

/// Format the list of installed packages for display.
///
/// Returns a multi-line string table with name, version, and size columns.
/// If no packages are installed, returns a short informational message.
pub fn format_package_list() -> String {
    let installed = REGISTRY.installed.lock();

    if installed.is_empty() {
        return String::from("No packages installed.\nUse 'pkg install <name>' to get started.\n");
    }

    let mut out = String::from("Installed packages:\n");
    out.push_str(&format!(
        "  {:<20} {:<12} {:>10}\n",
        "NAME", "VERSION", "SIZE"
    ));
    out.push_str(&format!("  {:-<20} {:-<12} {:->10}\n", "", "", ""));

    for pkg in installed.iter() {
        let size_str = if pkg.size_bytes >= 1024 {
            format!("{} KiB", pkg.size_bytes / 1024)
        } else {
            format!("{} B", pkg.size_bytes)
        };
        out.push_str(&format!(
            "  {:<20} {:<12} {:>10}\n",
            pkg.name, pkg.version, size_str
        ));
    }

    let total: u64 = installed.iter().map(|p| p.size_bytes).sum();
    out.push_str(&format!("\n  {} package(s), {} bytes total\n", installed.len(), total));
    out
}

/// Initialize the package subsystem.
///
/// Ensures the `/pkg` directory anchor exists in the VFS so that packages
/// can be installed later.
pub fn init() {
    if !vfs::exists("/pkg") {
        let _ = vfs::write("/pkg/.pkgindex", "MerlionOS package index\n");
    }
}
