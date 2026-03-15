/// AI Knowledge Base with vector similarity search for MerlionOS.
///
/// Provides an in-kernel document store where each document is represented
/// by a bag-of-words embedding vector. Documents can be added, deleted,
/// listed, and searched via cosine similarity over their embeddings.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::borrow::ToOwned;
use spin::Mutex;

/// Number of dimensions in the embedding vector.
const EMBED_DIM: usize = 128;

/// Auto-incrementing document ID counter.
static NEXT_ID: Mutex<u64> = Mutex::new(1);

/// Global knowledge base instance.
pub static KNOWLEDGE_BASE: Mutex<KnowledgeBase> = Mutex::new(KnowledgeBase::new());

/// A document stored in the knowledge base.
#[derive(Clone)]
pub struct Document {
    /// Unique document identifier.
    pub id: u64,
    /// Human-readable title.
    pub title: String,
    /// Full text content.
    pub content: String,
    /// Tags for categorical filtering.
    pub tags: Vec<String>,
    /// Bag-of-words embedding vector.
    pub embedding: Vec<i32>,
}

/// A search result pairing a document with its similarity score.
pub struct SearchResult {
    /// The matched document.
    pub document: Document,
    /// Cosine similarity score (scaled by 10 000 for integer math).
    pub score: i64,
}

/// In-kernel knowledge base backed by a vector of documents.
pub struct KnowledgeBase {
    /// All stored documents.
    documents: Vec<Document>,
}

impl KnowledgeBase {
    /// Create a new, empty knowledge base.
    pub const fn new() -> Self {
        Self {
            documents: Vec::new(),
        }
    }

    /// Add a document to the knowledge base.
    ///
    /// An embedding is computed automatically from the title and content.
    /// Returns the assigned document ID.
    pub fn add_document(&mut self, title: &str, content: &str, tags: Vec<String>) -> u64 {
        let id = {
            let mut next = NEXT_ID.lock();
            let current = *next;
            *next = current + 1;
            current
        };

        // Build combined text for embedding.
        let combined = format!("{} {}", title, content);
        let embedding = embed_text(&combined);

        let doc = Document {
            id,
            title: title.to_owned(),
            content: content.to_owned(),
            tags,
            embedding,
        };

        self.documents.push(doc);
        id
    }

    /// Search the knowledge base for documents most similar to `query`.
    ///
    /// Returns up to `top_k` results sorted by descending cosine similarity.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchResult> {
        let query_emb = embed_text(query);

        let mut results: Vec<SearchResult> = self
            .documents
            .iter()
            .map(|doc| {
                let score = cosine_similarity(&query_emb, &doc.embedding);
                SearchResult {
                    document: doc.clone(),
                    score,
                }
            })
            .collect();

        // Sort descending by score.
        results.sort_by(|a, b| b.score.cmp(&a.score));

        results.truncate(top_k);
        results
    }

    /// Delete a document by its ID.
    ///
    /// Returns `true` if the document was found and removed.
    pub fn delete_document(&mut self, id: u64) -> bool {
        let before = self.documents.len();
        self.documents.retain(|d| d.id != id);
        self.documents.len() < before
    }

    /// List all documents currently stored in the knowledge base.
    pub fn list_documents(&self) -> Vec<&Document> {
        self.documents.iter().collect()
    }

    /// Return the number of stored documents.
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Check whether the knowledge base is empty.
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}

/// Compute a bag-of-words embedding for the given text.
///
/// Each whitespace-delimited word is hashed to a dimension index in
/// `[0, EMBED_DIM)` and the corresponding counter is incremented.
pub fn embed_text(text: &str) -> Vec<i32> {
    let mut vec = vec![0i32; EMBED_DIM];

    for word in text.split_whitespace() {
        let dim = hash_word(word) % EMBED_DIM;
        vec[dim] = vec[dim].saturating_add(1);
    }

    vec
}

/// Compute cosine similarity between two embedding vectors.
///
/// Returns a value scaled by 10 000 (10 000 = identical, 0 = orthogonal).
pub fn cosine_similarity(a: &[i32], b: &[i32]) -> i64 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0;
    }

    let mut dot: i64 = 0;
    let mut mag_a: i64 = 0;
    let mut mag_b: i64 = 0;

    for i in 0..len {
        let ai = a[i] as i64;
        let bi = b[i] as i64;
        dot += ai * bi;
        mag_a += ai * ai;
        mag_b += bi * bi;
    }

    if mag_a == 0 || mag_b == 0 {
        return 0;
    }

    // Integer square root for magnitude computation.
    let denom = isqrt(mag_a) * isqrt(mag_b);
    if denom == 0 {
        return 0;
    }

    (dot * 10_000) / denom
}

/// Format search results into a human-readable string.
pub fn format_search_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::from("No matching documents found.");
    }

    let mut out = format!("Found {} result(s):\n", results.len());

    for (i, result) in results.iter().enumerate() {
        let doc = &result.document;
        let preview = if doc.content.len() > 80 {
            format!("{}...", &doc.content[..80])
        } else {
            doc.content.clone()
        };

        let tags_str = if doc.tags.is_empty() {
            String::from("none")
        } else {
            doc.tags.join(", ")
        };

        out += &format!(
            "\n  {}. [id={}] \"{}\" (score: {}, tags: {})\n     {}\n",
            i + 1,
            doc.id,
            doc.title,
            result.score,
            tags_str,
            preview,
        );
    }

    out
}

/// Simple DJB2-style hash of a word to a dimension index.
fn hash_word(word: &str) -> usize {
    let mut hash: u32 = 5381;
    for byte in word.bytes() {
        // Lowercase ASCII letters for case-insensitive matching.
        let b = if byte >= b'A' && byte <= b'Z' {
            byte + 32
        } else {
            byte
        };
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    hash as usize
}

/// Integer square root via Newton's method.
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}
