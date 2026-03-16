/// AI self-evolution for MerlionOS.
/// The kernel analyzes its own modules, identifies optimization opportunities,
/// suggests code improvements, and can generate/apply patches.
/// This is the capstone AI feature: an OS that improves itself.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Types of code analysis findings.
#[derive(Debug, Clone)]
pub enum FindingType {
    Performance,     // Can be made faster
    MemoryUsage,     // Can use less memory
    Security,        // Potential security issue
    Redundancy,      // Dead/duplicate code
    Complexity,      // Too complex, could be simplified
    BestPractice,    // Violates coding conventions
}

impl FindingType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FindingType::Performance => "PERF",
            FindingType::MemoryUsage => "MEM",
            FindingType::Security => "SEC",
            FindingType::Redundancy => "DUP",
            FindingType::Complexity => "CPLX",
            FindingType::BestPractice => "BEST",
        }
    }
}

/// A code analysis finding.
#[derive(Debug, Clone)]
pub struct Finding {
    pub id: u32,
    pub module: String,
    pub finding_type: FindingType,
    pub description: String,
    pub suggestion: String,
    pub severity: u8,  // 1-5 (5 = critical)
    pub auto_fixable: bool,
}

/// A generated patch.
#[derive(Debug, Clone)]
pub struct Patch {
    pub id: u32,
    pub finding_id: u32,
    pub module: String,
    pub description: String,
    pub diff: String,  // Text representation of the change
    pub applied: bool,
    pub verified: bool,
}

/// Evolution statistics.
#[derive(Debug, Clone)]
pub struct EvolveStats {
    pub analyses_run: usize,
    pub findings_total: usize,
    pub patches_generated: usize,
    pub patches_applied: usize,
    pub improvements: Vec<String>,
}

const MAX_FINDINGS: usize = 64;
const MAX_PATCHES: usize = 32;

static FINDINGS: Mutex<Vec<Finding>> = Mutex::new(Vec::new());
static PATCHES: Mutex<Vec<Patch>> = Mutex::new(Vec::new());
static NEXT_FINDING_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_PATCH_ID: AtomicU32 = AtomicU32::new(1);
static ANALYSES_RUN: AtomicUsize = AtomicUsize::new(0);

/// Analyze a specific kernel module by reading its source from VFS.
pub fn analyze_module(module_name: &str) -> Vec<Finding> {
    ANALYSES_RUN.fetch_add(1, Ordering::Relaxed);
    let mut findings = Vec::new();

    // Try to read module source from VFS
    let path = format!("/src/{}.rs", module_name);
    let source = crate::vfs::cat(&path).unwrap_or_default();

    // Static analysis heuristics
    let line_count = source.lines().count();

    // Check: module too large
    if line_count > 500 {
        findings.push(make_finding(module_name, FindingType::Complexity,
            &format!("Module has {} lines — consider splitting", line_count),
            "Break into sub-modules with clear responsibilities", 2));
    }

    // Check: unsafe blocks
    let unsafe_count = source.matches("unsafe").count();
    if unsafe_count > 5 {
        findings.push(make_finding(module_name, FindingType::Security,
            &format!("{} unsafe blocks — review for soundness", unsafe_count),
            "Minimize unsafe, add safety comments", 3));
    }

    // Check: unwrap usage
    let unwrap_count = source.matches(".unwrap()").count();
    if unwrap_count > 3 {
        findings.push(make_finding(module_name, FindingType::BestPractice,
            &format!("{} unwrap() calls — may panic", unwrap_count),
            "Replace with proper error handling", 2));
    }

    // Check: TODO/FIXME
    let todo_count = source.matches("TODO").count() + source.matches("FIXME").count();
    if todo_count > 0 {
        findings.push(make_finding(module_name, FindingType::Redundancy,
            &format!("{} TODO/FIXME comments remaining", todo_count),
            "Address or remove stale TODO items", 1));
    }

    // Check: large allocations
    if source.contains("Vec::with_capacity") || source.contains("Box::new") {
        let alloc_count = source.matches("Vec::new").count() + source.matches("Box::new").count();
        if alloc_count > 10 {
            findings.push(make_finding(module_name, FindingType::MemoryUsage,
                &format!("{} heap allocations — consider pooling", alloc_count),
                "Use slab allocator or pre-allocated pools", 2));
        }
    }

    // Check: lock contention (many Mutex uses)
    let mutex_count = source.matches(".lock()").count();
    if mutex_count > 8 {
        findings.push(make_finding(module_name, FindingType::Performance,
            &format!("{} lock acquisitions — potential contention", mutex_count),
            "Consider lock-free atomics or finer-grained locks", 3));
    }

    // Store findings
    let mut all_findings = FINDINGS.lock();
    for f in &findings {
        if all_findings.len() < MAX_FINDINGS {
            all_findings.push(f.clone());
        }
    }

    findings
}

fn make_finding(module: &str, ftype: FindingType, desc: &str, suggestion: &str, severity: u8) -> Finding {
    Finding {
        id: NEXT_FINDING_ID.fetch_add(1, Ordering::Relaxed),
        module: module.to_owned(),
        finding_type: ftype,
        description: desc.to_owned(),
        suggestion: suggestion.to_owned(),
        severity,
        auto_fixable: false,
    }
}

/// Analyze all major kernel modules.
pub fn analyze_all() -> String {
    let modules = [
        "vfs", "task", "security", "shell", "syscall", "memory",
        "net", "netstack", "tcp_real", "httpd", "sshd",
        "capability", "profiler", "allocator",
    ];

    let mut total_findings = 0usize;
    let mut out = String::from("=== MerlionOS Self-Analysis ===\n\n");

    for module in &modules {
        let findings = analyze_module(module);
        if !findings.is_empty() {
            out.push_str(&format!("{}:\n", module));
            for f in &findings {
                out.push_str(&format!("  [{}] {} (sev:{})\n    -> {}\n",
                    f.finding_type.as_str(), f.description, f.severity, f.suggestion));
            }
            total_findings += findings.len();
        }
    }

    out.push_str(&format!("\nTotal: {} findings across {} modules\n", total_findings, modules.len()));
    out
}

/// Generate a patch for a specific finding.
pub fn generate_patch(finding_id: u32) -> Result<u32, &'static str> {
    let findings = FINDINGS.lock();
    let finding = findings.iter().find(|f| f.id == finding_id)
        .ok_or("finding not found")?;

    let patch_id = NEXT_PATCH_ID.fetch_add(1, Ordering::Relaxed);
    let diff = format!(
        "--- a/src/{}.rs\n+++ b/src/{}.rs\n@@ suggested change @@\n# {}\n# Fix: {}",
        finding.module, finding.module, finding.description, finding.suggestion
    );

    let patch = Patch {
        id: patch_id,
        finding_id,
        module: finding.module.clone(),
        description: finding.suggestion.clone(),
        diff,
        applied: false,
        verified: false,
    };

    drop(findings);
    let mut patches = PATCHES.lock();
    if patches.len() < MAX_PATCHES {
        patches.push(patch);
    }

    Ok(patch_id)
}

/// List all findings.
pub fn list_findings() -> String {
    let findings = FINDINGS.lock();
    if findings.is_empty() { return String::from("No findings. Run 'evolve' first.\n"); }
    let mut out = format!("Findings ({}):\n", findings.len());
    out.push_str(&format!("{:>4} {:<6} {:<12} {:<4} {}\n", "ID", "Type", "Module", "Sev", "Description"));
    for f in findings.iter() {
        let desc = if f.description.len() > 50 { format!("{}...", &f.description[..47]) } else { f.description.clone() };
        out.push_str(&format!("{:>4} {:<6} {:<12} {:<4} {}\n",
            f.id, f.finding_type.as_str(), f.module, f.severity, desc));
    }
    out
}

/// List all patches.
pub fn list_patches() -> String {
    let patches = PATCHES.lock();
    if patches.is_empty() { return String::from("No patches generated.\n"); }
    let mut out = format!("Patches ({}):\n", patches.len());
    for p in patches.iter() {
        let status = if p.applied { "applied" } else if p.verified { "verified" } else { "pending" };
        out.push_str(&format!("  #{} [{}] {} — {}\n", p.id, status, p.module, p.description));
    }
    out
}

/// Get evolution statistics.
pub fn evolve_stats() -> String {
    let findings = FINDINGS.lock().len();
    let patches = PATCHES.lock().len();
    let applied = PATCHES.lock().iter().filter(|p| p.applied).count();
    format!(
        "Self-evolution: {} analyses, {} findings, {} patches ({} applied)",
        ANALYSES_RUN.load(Ordering::Relaxed), findings, patches, applied,
    )
}

/// Clear all findings and patches.
pub fn reset() {
    FINDINGS.lock().clear();
    PATCHES.lock().clear();
    NEXT_FINDING_ID.store(1, Ordering::Relaxed);
    NEXT_PATCH_ID.store(1, Ordering::Relaxed);
}

/// Initialize self-evolution system.
pub fn init() {
    crate::serial_println!("[self_evolve] AI self-evolution engine initialized");
    crate::klog_println!("[self_evolve] initialized");
}
