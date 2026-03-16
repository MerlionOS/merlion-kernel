//! Text diffing algorithm for MerlionOS.
//!
//! Line-level differencing via longest common subsequence, with unified and
//! colored output formats, patch application, and diff statistics.

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A single diff operation on one line of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    /// Line present in both old and new.
    Equal(String),
    /// Line added in the new text.
    Insert(String),
    /// Line removed from the old text.
    Delete(String),
}

/// Splits text into lines, stripping a single trailing newline.
fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() { return Vec::new(); }
    text.strip_suffix('\n').unwrap_or(text).split('\n').collect()
}

/// Builds the LCS length table for two line slices.
fn lcs_table(old: &[&str], new: &[&str]) -> Vec<Vec<usize>> {
    let (rows, cols) = (old.len() + 1, new.len() + 1);
    let mut t = vec![vec![0usize; cols]; rows];
    for i in 1..rows {
        for j in 1..cols {
            t[i][j] = if old[i - 1] == new[j - 1] {
                t[i - 1][j - 1] + 1
            } else {
                core::cmp::max(t[i - 1][j], t[i][j - 1])
            };
        }
    }
    t
}

/// Backtracks the LCS table into a [`DiffOp`] sequence.
fn backtrack(t: &[Vec<usize>], old: &[&str], new: &[&str]) -> Vec<DiffOp> {
    let (mut i, mut j) = (old.len(), new.len());
    let mut ops = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            ops.push(DiffOp::Equal(String::from(old[i - 1])));
            i -= 1; j -= 1;
        } else if j > 0 && (i == 0 || t[i][j - 1] >= t[i - 1][j]) {
            ops.push(DiffOp::Insert(String::from(new[j - 1])));
            j -= 1;
        } else {
            ops.push(DiffOp::Delete(String::from(old[i - 1])));
            i -= 1;
        }
    }
    ops.reverse();
    ops
}

/// Computes a line-level diff between `old` and `new` using LCS.
///
/// Returns the minimal [`DiffOp`] sequence to transform `old` into `new`.
///
/// # Examples
/// ```
/// let ops = diff::diff_lines("a\nb\nc\n", "a\nc\nd\n");
/// ```
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffOp> {
    let (ol, nl) = (split_lines(old), split_lines(new));
    let t = lcs_table(&ol, &nl);
    backtrack(&t, &ol, &nl)
}

/// Pushes the decimal representation of `n` onto `buf` (no_std helper).
fn push_usize(buf: &mut String, mut n: usize) {
    if n == 0 { buf.push('0'); return; }
    let start = buf.len();
    while n > 0 { buf.push((b'0' + (n % 10) as u8) as char); n /= 10; }
    { let v = unsafe { buf.as_mut_vec() }; v[start..].reverse(); }
}

/// Formats diff operations as a unified diff string.
///
/// Each group of changes is surrounded by `context_lines` lines of equal
/// context and preceded by an `@@ -o,c +n,c @@` header. Lines are prefixed
/// with `' '`, `'+'`, or `'-'`.
pub fn format_unified(ops: &[DiffOp], context_lines: usize) -> String {
    let changes: Vec<usize> = ops.iter().enumerate()
        .filter(|(_, op)| !matches!(op, DiffOp::Equal(_)))
        .map(|(i, _)| i).collect();
    if changes.is_empty() { return String::new(); }

    // Build hunks by merging overlapping context windows.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let (mut hs, mut he) = (
        changes[0].saturating_sub(context_lines),
        core::cmp::min(changes[0] + context_lines, ops.len() - 1),
    );
    for &ci in &changes[1..] {
        let cs = ci.saturating_sub(context_lines);
        let ce = core::cmp::min(ci + context_lines, ops.len() - 1);
        if cs <= he + 1 { he = ce; } else { hunks.push((hs, he)); hs = cs; he = ce; }
    }
    hunks.push((hs, he));

    let mut out = String::new();
    for (start, end) in hunks {
        let (mut os, mut ns) = (1usize, 1usize);
        for op in &ops[..start] {
            match op {
                DiffOp::Equal(_) => { os += 1; ns += 1; }
                DiffOp::Delete(_) => os += 1,
                DiffOp::Insert(_) => ns += 1,
            }
        }
        let (mut oc, mut nc) = (0usize, 0usize);
        for op in &ops[start..=end] {
            match op {
                DiffOp::Equal(_) => { oc += 1; nc += 1; }
                DiffOp::Delete(_) => oc += 1,
                DiffOp::Insert(_) => nc += 1,
            }
        }
        out.push_str("@@ -"); push_usize(&mut out, os); out.push(',');
        push_usize(&mut out, oc); out.push_str(" +"); push_usize(&mut out, ns);
        out.push(','); push_usize(&mut out, nc); out.push_str(" @@\n");

        for op in &ops[start..=end] {
            let (prefix, line) = match op {
                DiffOp::Equal(l)  => (' ', l.as_str()),
                DiffOp::Insert(l) => ('+', l.as_str()),
                DiffOp::Delete(l) => ('-', l.as_str()),
            };
            out.push(prefix); out.push_str(line); out.push('\n');
        }
    }
    out
}

/// Formats diff operations with ANSI color codes.
///
/// Inserted lines are **green** (`\x1b[32m`), deleted lines are **red**
/// (`\x1b[31m`), and equal lines have no color. Each colored line ends with
/// the reset sequence `\x1b[0m`.
pub fn format_colored(ops: &[DiffOp]) -> String {
    let mut out = String::new();
    for op in ops {
        match op {
            DiffOp::Equal(l) => {
                out.push(' '); out.push_str(l); out.push('\n');
            }
            DiffOp::Insert(l) => {
                out.push_str("\x1b[32m+"); out.push_str(l);
                out.push_str("\x1b[0m\n");
            }
            DiffOp::Delete(l) => {
                out.push_str("\x1b[31m-"); out.push_str(l);
                out.push_str("\x1b[0m\n");
            }
        }
    }
    out
}

/// Applies a diff to `original`, producing the new text.
///
/// `Equal` and `Insert` lines are kept; `Delete` lines are dropped. The
/// caller must ensure the ops were generated from the same `original`.
pub fn apply_patch(original: &str, ops: &[DiffOp]) -> String {
    let _ = original;
    let mut out = String::new();
    for op in ops {
        match op {
            DiffOp::Equal(l) | DiffOp::Insert(l) => {
                out.push_str(l); out.push('\n');
            }
            DiffOp::Delete(_) => {}
        }
    }
    out
}

/// Returns `(insertions, deletions, unchanged)` counts for a diff.
///
/// # Examples
/// ```
/// let ops = diff::diff_lines("a\nb\n", "a\nc\n");
/// let (ins, del, eq) = diff::stats(&ops);
/// assert_eq!((ins, del, eq), (1, 1, 1));
/// ```
pub fn stats(ops: &[DiffOp]) -> (usize, usize, usize) {
    let (mut ins, mut del, mut eq) = (0, 0, 0);
    for op in ops {
        match op {
            DiffOp::Insert(_) => ins += 1,
            DiffOp::Delete(_) => del += 1,
            DiffOp::Equal(_)  => eq += 1,
        }
    }
    (ins, del, eq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inputs() { assert!(diff_lines("", "").is_empty()); }

    #[test]
    fn identical_inputs() {
        let ops = diff_lines("a\nb\nc\n", "a\nb\nc\n");
        assert_eq!(stats(&ops), (0, 0, 3));
    }

    #[test]
    fn pure_insertion() {
        let ops = diff_lines("", "a\nb\n");
        assert!(ops.iter().all(|op| matches!(op, DiffOp::Insert(_))));
    }

    #[test]
    fn pure_deletion() {
        let ops = diff_lines("a\nb\n", "");
        assert!(ops.iter().all(|op| matches!(op, DiffOp::Delete(_))));
    }

    #[test]
    fn mixed_diff_stats() {
        assert_eq!(stats(&diff_lines("a\nb\nc\n", "a\nc\nd\n")), (1, 1, 2));
    }

    #[test]
    fn apply_patch_roundtrip() {
        let old = "alpha\nbeta\ngamma\n";
        let new = "alpha\ndelta\ngamma\nepsilon\n";
        assert_eq!(apply_patch(old, &diff_lines(old, new)), new);
    }

    #[test]
    fn unified_format() {
        let u = format_unified(&diff_lines("a\nb\n", "a\nc\n"), 3);
        assert!(u.contains("@@") && u.contains("-b") && u.contains("+c"));
    }

    #[test]
    fn colored_format() {
        let c = format_colored(&diff_lines("a\n", "b\n"));
        assert!(c.contains("\x1b[31m") && c.contains("\x1b[32m"));
    }

    #[test]
    fn push_usize_cases() {
        let mut b = String::new(); push_usize(&mut b, 0); assert_eq!(b, "0");
        b.clear(); push_usize(&mut b, 12345); assert_eq!(b, "12345");
    }
}
