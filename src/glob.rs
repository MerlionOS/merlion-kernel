/// Glob/wildcard pattern matcher for the MerlionOS shell.
///
/// Syntax: `*` (any chars), `?` (single char), `[abc]` (char set),
/// `[!abc]` (negated set), `[a-z]` (range), `**` (recursive directory match).
/// Entry points: [`glob_match`] and [`glob_files`].

use alloc::string::String;
use alloc::vec::Vec;

/// Match a glob `pattern` against `text`, returning `true` on a full match.
///
/// The `**` wildcard matches zero or more path components including `/`.
/// Single `*` does not cross `/` boundaries.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    match_recursive(pattern.as_bytes(), text.as_bytes())
}

/// Walk the VFS and return every absolute path matching `pattern`.
///
/// Accepts absolute or relative patterns (relative patterns are anchored to
/// `/`). Returns an empty vector if the VFS is uninitialised or pattern is empty.
pub fn glob_files(pattern: &str) -> Vec<String> {
    if pattern.is_empty() {
        return Vec::new();
    }

    let abs_pattern = if pattern.starts_with('/') {
        String::from(pattern)
    } else {
        let mut s = String::from("/");
        s.push_str(pattern);
        s
    };

    let mut results = Vec::new();
    collect_paths(&mut results, "/");
    results
        .into_iter()
        .filter(|path| glob_match(&abs_pattern, path))
        .collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Core matching engine operating on byte slices with backtracking.
fn match_recursive(pat: &[u8], txt: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (plen, tlen) = (pat.len(), txt.len());

    // Saved positions for backtracking on `*`
    let mut star_pi: Option<usize> = None;
    let mut star_ti: usize = 0;

    while ti < tlen {
        // Handle `**` — matches zero or more path segments including `/`.
        if pi + 1 < plen && pat[pi] == b'*' && pat[pi + 1] == b'*' {
            // Skip the `**` and any following `/`
            let mut pp = pi + 2;
            if pp < plen && pat[pp] == b'/' {
                pp += 1;
            }
            // Try matching the remainder against every suffix of text
            let mut t = ti;
            loop {
                if match_recursive(&pat[pp..], &txt[t..]) {
                    return true;
                }
                if t >= tlen {
                    break;
                }
                t += 1;
            }
            return false;
        }

        if pi < plen && pat[pi] == b'?' && txt[ti] != b'/' {
            pi += 1;
            ti += 1;
            continue;
        }

        if pi < plen && pat[pi] == b'*' {
            // Single `*` — does not cross `/`
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
            continue;
        }

        if pi < plen && pat[pi] == b'[' {
            if let Some((matched, advance)) = match_class(&pat[pi..], txt[ti]) {
                if matched {
                    pi += advance;
                    ti += 1;
                    continue;
                }
            }
            // Fall through to backtrack
        } else if pi < plen && pat[pi] == txt[ti] {
            pi += 1;
            ti += 1;
            continue;
        }

        // Backtrack on a previous `*`
        if let Some(sp) = star_pi {
            star_ti += 1;
            // `*` must not cross `/`
            if txt[star_ti - 1] == b'/' {
                return false;
            }
            ti = star_ti;
            pi = sp + 1;
            continue;
        }

        return false;
    }

    // Consume trailing `*` / `**` in the pattern
    while pi < plen && pat[pi] == b'*' {
        pi += 1;
    }

    pi == plen
}

/// Parse and evaluate a `[…]` character class at the start of `pat`.
/// Returns `Some((matched, bytes_consumed))` or `None` if malformed.
fn match_class(pat: &[u8], ch: u8) -> Option<(bool, usize)> {
    if pat.is_empty() || pat[0] != b'[' {
        return None;
    }

    let mut i = 1;
    let negated = if i < pat.len() && pat[i] == b'!' {
        i += 1;
        true
    } else {
        false
    };

    let mut matched = false;

    while i < pat.len() && pat[i] != b']' {
        // Range: a-z
        if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
            let lo = pat[i];
            let hi = pat[i + 2];
            if ch >= lo && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if pat[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }

    // Must close with `]`
    if i >= pat.len() || pat[i] != b']' {
        return None;
    }

    let result = if negated { !matched } else { matched };
    Some((result, i + 1))
}

/// Recursively collect every path in the VFS starting from `base`.
fn collect_paths(out: &mut Vec<String>, base: &str) {
    out.push(String::from(base));

    let entries = match crate::vfs::ls(base) {
        Ok(v) => v,
        Err(_) => return,
    };

    for (name, kind) in &entries {
        let child = if base == "/" {
            let mut s = String::from("/");
            s.push_str(name);
            s
        } else {
            let mut s = String::from(base);
            s.push('/');
            s.push_str(name);
            s
        };

        if *kind == 'd' {
            collect_paths(out, &child);
        } else {
            out.push(child);
        }
    }
}
