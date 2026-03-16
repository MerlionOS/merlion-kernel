/// Full-text search engine for MerlionOS VFS files.
///
/// Provides an inverted index that maps words to file paths, enabling fast
/// full-text search across the entire virtual filesystem. Supports relevance
/// scoring, snippet extraction, and ANSI-highlighted match output.
use alloc::string::String;
use alloc::vec::Vec;
use alloc::borrow::ToOwned;
use alloc::format;
use spin::Mutex;

/// Maximum number of unique terms the index will track.
const MAX_TERMS: usize = 1024;
/// Maximum number of files per term in the posting list.
const MAX_POSTINGS: usize = 128;
/// Number of context characters shown around a match in snippets.
const SNIPPET_CONTEXT: usize = 40;
/// ANSI escape code for bold red (highlight).
const ANSI_HIGHLIGHT_ON: &str = "\x1b[1;31m";
/// ANSI escape code to reset formatting.
const ANSI_RESET: &str = "\x1b[0m";

/// A single entry in the inverted index: a term and the file paths containing it.
struct IndexEntry {
    term: String,
    postings: Vec<String>,
}

/// Inverted index mapping words to the VFS paths that contain them.
struct SearchIndex {
    entries: Vec<IndexEntry>,
}

/// Result of a search query, returned to the caller.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// VFS path of the matching file.
    pub path: String,
    /// Relevance score (higher is better). Based on term frequency.
    pub score: u32,
    /// Short snippet from the file showing the match in context.
    pub snippet: String,
}

/// Global search index, protected by a spinlock for concurrent access.
static INDEX: Mutex<Option<SearchIndex>> = Mutex::new(None);

impl SearchIndex {
    /// Create a new, empty search index.
    fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Clear all entries from the index.
    fn clear(&mut self) {
        self.entries.clear();
    }

    /// Add a (term, path) pair to the inverted index.
    fn add_posting(&mut self, term: &str, path: &str) {
        for entry in self.entries.iter_mut() {
            if entry.term == term {
                if !entry.postings.iter().any(|p| p == path) {
                    if entry.postings.len() < MAX_POSTINGS {
                        entry.postings.push(path.to_owned());
                    }
                }
                return;
            }
        }
        if self.entries.len() < MAX_TERMS {
            let mut postings = Vec::new();
            postings.push(path.to_owned());
            self.entries.push(IndexEntry { term: term.to_owned(), postings });
        }
    }

    /// Look up a term and return the list of paths that contain it.
    fn lookup(&self, term: &str) -> Option<&Vec<String>> {
        for entry in &self.entries {
            if entry.term == term {
                return Some(&entry.postings);
            }
        }
        None
    }
}

/// Tokenize text into lowercase words with punctuation stripped.
///
/// Splits on whitespace, converts to lowercase, and removes any character
/// that is not alphanumeric or an underscore.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Index a single file given its VFS path and content.
///
/// Tokenizes the content and adds each unique term to the global inverted
/// index, associated with the given path.
pub fn index_file(path: &str, content: &str) {
    let mut idx = INDEX.lock();
    let index = idx.get_or_insert_with(SearchIndex::new);
    let tokens = tokenize(content);
    let mut seen: Vec<String> = Vec::new();
    for token in &tokens {
        if !seen.iter().any(|s| s == token) {
            index.add_posting(token, path);
            seen.push(token.clone());
        }
    }
}

/// Recursively walk a VFS directory and index all regular files.
///
/// Reads each file via `crate::vfs::cat` and indexes its content. Recurses
/// into subdirectories. Skips device and proc nodes.
pub fn index_directory(path: &str) {
    let entries = match crate::vfs::ls(path) {
        Ok(e) => e,
        Err(_) => return,
    };
    for (name, type_char) in &entries {
        let full_path = if path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", path, name)
        };
        match *type_char {
            'd' => index_directory(&full_path),
            '-' => {
                if let Ok(content) = crate::vfs::cat(&full_path) {
                    index_file(&full_path, &content);
                }
            }
            _ => {} // device or proc node — skip
        }
    }
}

/// Search the index for files matching the given query.
///
/// The query is tokenized the same way file content is. Each query term is
/// looked up in the inverted index. Files are scored by the number of query
/// terms they match. Results are sorted by descending score.
pub fn search(query: &str) -> Vec<SearchResult> {
    let idx = INDEX.lock();
    let index = match idx.as_ref() {
        Some(i) => i,
        None => return Vec::new(),
    };
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return Vec::new();
    }

    // Accumulate per-path scores.
    let mut scores: Vec<(String, u32)> = Vec::new();
    for token in &query_tokens {
        if let Some(postings) = index.lookup(token) {
            for path in postings {
                let mut found = false;
                for entry in scores.iter_mut() {
                    if entry.0 == *path {
                        entry.1 += 1;
                        found = true;
                        break;
                    }
                }
                if !found {
                    scores.push((path.clone(), 1));
                }
            }
        }
    }
    scores.sort_by(|a, b| b.1.cmp(&a.1));

    // Drop lock before reading file contents for snippets.
    let scored = scores;
    drop(idx);

    scored
        .iter()
        .map(|(path, score)| {
            let snippet = if let Ok(content) = crate::vfs::cat(path) {
                extract_snippet(&content, &query_tokens)
            } else {
                String::new()
            };
            SearchResult { path: path.clone(), score: *score, snippet }
        })
        .collect()
}

/// Extract a short snippet from file content around the first query match.
///
/// Finds the first occurrence of any query token and returns a window of
/// `SNIPPET_CONTEXT` characters on each side.
fn extract_snippet(content: &str, query_tokens: &[String]) -> String {
    let lower = content.to_lowercase();
    let mut best_pos: Option<usize> = None;
    for token in query_tokens {
        if let Some(pos) = lower.find(token.as_str()) {
            if best_pos.is_none() || pos < best_pos.unwrap() {
                best_pos = Some(pos);
            }
        }
    }
    let pos = match best_pos {
        Some(p) => p,
        None => return String::new(),
    };
    let start = if pos > SNIPPET_CONTEXT { pos - SNIPPET_CONTEXT } else { 0 };
    let end = core::cmp::min(content.len(), pos + SNIPPET_CONTEXT);
    let mut snippet = String::new();
    if start > 0 {
        snippet.push_str("...");
    }
    snippet.push_str(&content[start..end]);
    if end < content.len() {
        snippet.push_str("...");
    }
    snippet
}

/// Highlight all occurrences of query terms in text using ANSI escape codes.
///
/// Each matching word is wrapped in bold red ANSI codes. Matching is
/// case-insensitive.
pub fn highlight_matches(text: &str, query: &str) -> String {
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return text.to_owned();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = String::new();
    for segment in words {
        let lower_clean: String = segment
            .trim()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .to_lowercase();
        if query_tokens.iter().any(|t| *t == lower_clean) {
            let leading: String = segment.chars().take_while(|c| c.is_whitespace()).collect();
            let trailing: String = segment.chars().rev().take_while(|c| c.is_whitespace()).collect();
            result.push_str(&leading);
            result.push_str(ANSI_HIGHLIGHT_ON);
            result.push_str(segment.trim());
            result.push_str(ANSI_RESET);
            result.push_str(&trailing);
        } else {
            result.push_str(segment);
        }
    }
    result
}

/// Rebuild the entire search index from scratch.
///
/// Clears the current index and re-walks the VFS from root, indexing
/// every regular file.
pub fn rebuild_index() {
    {
        let mut idx = INDEX.lock();
        let index = idx.get_or_insert_with(SearchIndex::new);
        index.clear();
    }
    index_directory("/");
}

/// Initialize the search subsystem by building the index for the first time.
pub fn init() {
    rebuild_index();
}
