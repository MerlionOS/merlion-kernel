/// Vector store for semantic search in MerlionOS.
/// Stores document embeddings as fixed-point i32 vectors and provides
/// cosine similarity search for knowledge retrieval.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const SCALE: i32 = 256;
const EMBEDDING_DIM: usize = 32;  // Compact embedding dimension
const MAX_DOCUMENTS: usize = 256;

/// A document with its vector embedding.
#[derive(Debug, Clone)]
pub struct VectorDoc {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub embedding: Vec<i32>,  // Fixed-point vector [EMBEDDING_DIM]
    pub metadata: Vec<(String, String)>,  // Key-value metadata
    pub added_tick: u64,
}

/// Search result with similarity score.
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub doc_id: u64,
    pub title: String,
    pub score: i32,  // Cosine similarity * SCALE
    pub snippet: String,
}

static NEXT_DOC_ID: AtomicU64 = AtomicU64::new(1);
static STORE: Mutex<Vec<VectorDoc>> = Mutex::new(Vec::new());
static SEARCH_COUNT: AtomicUsize = AtomicUsize::new(0);
static INDEX_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Generate a simple embedding from text content.
/// Uses a hash-based approach: for each word, hash it and scatter into the embedding vector.
/// This is a simplified bag-of-words embedding suitable for a hobby OS.
pub fn embed(text: &str) -> Vec<i32> {
    let mut vec = alloc::vec![0i32; EMBEDDING_DIM];

    for word in text.split_whitespace() {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric());
        if word.is_empty() { continue; }

        // FNV-1a hash of the word
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in word.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x00000100000001B3);
        }

        // Scatter into embedding dimensions
        let dim = (hash % EMBEDDING_DIM as u64) as usize;
        let sign = if (hash >> 32) & 1 == 0 { 1 } else { -1 };
        vec[dim] += sign * SCALE;

        // Also affect neighboring dimensions for richer representation
        let dim2 = ((hash >> 8) % EMBEDDING_DIM as u64) as usize;
        vec[dim2] += sign * (SCALE / 2);
    }

    // Normalize (approximate L2 norm)
    let norm_sq: i64 = vec.iter().map(|&x| (x as i64) * (x as i64)).sum();
    if norm_sq > 0 {
        // Integer square root approximation
        let mut norm = 1i64;
        let mut n = norm_sq;
        while n > 0 { n >>= 2; norm <<= 1; }
        // Newton's method (2 iterations)
        for _ in 0..3 {
            if norm > 0 { norm = (norm + norm_sq / norm) / 2; }
        }
        if norm > 0 {
            for v in vec.iter_mut() {
                *v = ((*v as i64 * SCALE as i64) / norm) as i32;
            }
        }
    }

    vec
}

/// Cosine similarity between two vectors (returns fixed-point i32, SCALE=256).
pub fn cosine_similarity(a: &[i32], b: &[i32]) -> i32 {
    let len = a.len().min(b.len());
    let mut dot: i64 = 0;
    let mut norm_a: i64 = 0;
    let mut norm_b: i64 = 0;

    for i in 0..len {
        dot += a[i] as i64 * b[i] as i64;
        norm_a += a[i] as i64 * a[i] as i64;
        norm_b += b[i] as i64 * b[i] as i64;
    }

    if norm_a == 0 || norm_b == 0 { return 0; }

    // Approximate: dot / (sqrt(norm_a) * sqrt(norm_b)) * SCALE
    // Use integer sqrt
    let sqrt_a = isqrt(norm_a);
    let sqrt_b = isqrt(norm_b);
    let denom = sqrt_a * sqrt_b;

    if denom == 0 { return 0; }
    ((dot * SCALE as i64) / denom) as i32
}

/// Integer square root.
fn isqrt(n: i64) -> i64 {
    if n <= 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Add a document to the vector store.
pub fn add_document(title: &str, content: &str, metadata: Vec<(String, String)>) -> u64 {
    let id = NEXT_DOC_ID.fetch_add(1, Ordering::Relaxed);
    let embedding = embed(content);

    let mut store = STORE.lock();
    if store.len() >= MAX_DOCUMENTS {
        store.remove(0);  // Evict oldest
    }
    store.push(VectorDoc {
        id, title: title.to_owned(), content: content.to_owned(),
        embedding, metadata, added_tick: crate::timer::ticks(),
    });

    INDEX_COUNT.fetch_add(1, Ordering::Relaxed);
    id
}

/// Search for documents similar to the query text.
pub fn search(query: &str, top_k: usize) -> Vec<VectorSearchResult> {
    SEARCH_COUNT.fetch_add(1, Ordering::Relaxed);
    let query_embedding = embed(query);

    let store = STORE.lock();
    let mut scored: Vec<(usize, i32)> = store.iter().enumerate()
        .map(|(i, doc)| (i, cosine_similarity(&query_embedding, &doc.embedding)))
        .collect();

    // Sort by descending score
    scored.sort_by(|a, b| b.1.cmp(&a.1));

    scored.iter()
        .take(top_k)
        .filter(|(_, score)| *score > 0)
        .map(|(idx, score)| {
            let doc = &store[*idx];
            let snippet = if doc.content.len() > 80 {
                format!("{}...", &doc.content[..77])
            } else {
                doc.content.clone()
            };
            VectorSearchResult {
                doc_id: doc.id,
                title: doc.title.clone(),
                score: *score,
                snippet,
            }
        })
        .collect()
}

/// Format search results as text.
pub fn format_results(results: &[VectorSearchResult]) -> String {
    if results.is_empty() {
        return String::from("No matching documents found.\n");
    }
    let mut out = format!("Found {} results:\n", results.len());
    for (i, r) in results.iter().enumerate() {
        let score_pct = (r.score * 100) / SCALE;
        out.push_str(&format!("  {}. [{}%] {} (id:{})\n     {}\n",
            i + 1, score_pct, r.title, r.doc_id, r.snippet));
    }
    out
}

/// List all documents in the store.
pub fn list_documents() -> String {
    let store = STORE.lock();
    if store.is_empty() {
        return String::from("Vector store is empty.\n");
    }
    let mut out = format!("Vector store ({} documents):\n", store.len());
    for doc in store.iter() {
        out.push_str(&format!("  [{}] {} ({} chars)\n", doc.id, doc.title, doc.content.len()));
    }
    out
}

/// Delete a document by ID.
pub fn delete_document(id: u64) -> bool {
    let mut store = STORE.lock();
    let len = store.len();
    store.retain(|d| d.id != id);
    store.len() < len
}

/// Get store statistics.
pub fn store_stats() -> String {
    let count = STORE.lock().len();
    format!(
        "Vector store: {} documents, dim={}, {} searches, {} indexed",
        count, EMBEDDING_DIM,
        SEARCH_COUNT.load(Ordering::Relaxed),
        INDEX_COUNT.load(Ordering::Relaxed),
    )
}

/// Initialize with some built-in kernel knowledge.
pub fn init() {
    add_document("MerlionOS", "MerlionOS is an AI-native operating system written in Rust for x86_64. Born for AI, Built by AI.", Vec::new());
    add_document("Memory Management", "The kernel uses a linked-list heap allocator with 64KB initial size. Page tables use 4-level paging with 4KB pages. Slab allocator for fixed-size objects.", Vec::new());
    add_document("Process Model", "Tasks share the kernel address space. User processes get separate page tables. Preemptive round-robin scheduling with 100Hz timer.", Vec::new());
    add_document("Networking", "TCP/IP stack with e1000e and virtio-net drivers. UDP, ARP, ICMP, DHCP, DNS, HTTP, TLS, WebSocket, MQTT support.", Vec::new());
    add_document("Security", "Unix-style rwx permissions, user/group management, capability-based security with 14 flags, seccomp-like syscall filtering, audit logging.", Vec::new());

    crate::serial_println!("[vector_store] initialized with {} documents", STORE.lock().len());
    crate::klog_println!("[vector_store] initialized");
}
