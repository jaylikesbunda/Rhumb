//! Search: instant in-directory filtering plus a cancellable recursive walk.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use crate::fs_model::Entry;
use crate::workers::{Msg, SearchChunk};

/// Maximum hits surfaced for a recursive search.
pub const MAX_RESULTS: usize = 20_000;
/// Depth limit for a recursive search.
pub const MAX_DEPTH: usize = 24;
/// Wall-clock limit for a recursive search.
pub const MAX_SECONDS: u64 = 30;

/// Does `name` match every term in `query`?
///
/// Matching is case-insensitive and allocation-free on the hot path, because
/// this runs once per directory entry while the user types. Terms may appear
/// in any order, so `md cargo` matches `cargo-notes.md`.
pub fn matches(name: &str, query: &str) -> bool {
    query
        .split_whitespace()
        .all(|term| !term.is_empty() && contains_ignore_case(name, term))
}

/// Case-insensitive substring search without allocating.
pub fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay = haystack.as_bytes();
    let ned = needle.as_bytes();
    if ned.len() > hay.len() {
        return false;
    }
    if ned.len() == 1 {
        let n = ned[0].to_ascii_lowercase();
        return hay.iter().any(|b| b.to_ascii_lowercase() == n);
    }
    // Fast path: pure ASCII, which covers nearly every file name.
    if hay.is_ascii() && ned.is_ascii() {
        let first = ned[0].to_ascii_lowercase();
        let last = ned[ned.len() - 1].to_ascii_lowercase();
        for start in 0..=(hay.len() - ned.len()) {
            if hay[start].to_ascii_lowercase() != first {
                continue;
            }
            if hay[start + ned.len() - 1].to_ascii_lowercase() != last {
                continue;
            }
            if hay[start + 1..start + ned.len() - 1]
                .iter()
                .zip(&ned[1..ned.len() - 1])
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
            {
                return true;
            }
        }
        return false;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Lower-cases a query once, for repeated matching during a walk.
pub fn prepare(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Matches a name against pre-lowercased terms.
pub fn matches_prepared(name: &str, terms: &[String]) -> bool {
    terms.iter().all(|t| contains_ignore_case(name, t))
}

/// Live state of a recursive search.
#[derive(Default)]
pub struct Search {
    pub query: String,
    pub running: bool,
    pub results: Vec<Entry>,
    pub scanned: u64,
    pub truncated: bool,
    cancel: Option<Arc<AtomicBool>>,
}

impl Search {
    /// Requests cancellation; the worker notices on its next item.
    pub fn cancel(&mut self) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
        }
        self.running = false;
    }

    /// Starts (or restarts) a search rooted at `root`.
    pub fn start(&mut self, root: &Path, query: &str, tx: Sender<Msg>) {
        self.cancel();
        self.query = query.to_owned();
        self.results.clear();
        self.scanned = 0;
        self.truncated = false;

        let terms = prepare(query);
        if terms.is_empty() || !root.is_dir() {
            self.running = false;
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.running = true;

        let root = root.to_path_buf();

        std::thread::Builder::new()
            .name("xplor-search".into())
            .spawn(move || {
                let started = Instant::now();
                let mut found: Vec<Entry> = Vec::new();
                let mut scanned: u64 = 0;
                let mut pending: Vec<Entry> = Vec::new();
                let mut truncated = false;

                'walk: for entry in walkdir::WalkDir::new(&root)
                    .max_depth(MAX_DEPTH)
                    .follow_links(false)
                    .into_iter()
                    .filter_entry(|e| !skip_entry(e.path(), &root))
                    .filter_map(Result::ok)
                {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    if started.elapsed().as_secs() > MAX_SECONDS {
                        truncated = true;
                        break;
                    }
                    scanned += 1;
                    let path = entry.path();
                    if path == root {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if matches_prepared(name.as_str(), &terms) {
                        if found.len() >= MAX_RESULTS {
                            truncated = true;
                            break 'walk;
                        }
                        // Metadata is read here, on the worker, so the UI never
                        // touches the disk while painting rows.
                        let meta = entry.metadata().ok();
                        let e = Entry {
                            name: entry.file_name().to_string_lossy().to_string(),
                            path: path.to_path_buf(),
                            is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                            is_symlink: meta.as_ref().is_some_and(|m| m.is_symlink()),
                            size: meta.as_ref().map_or(0, |m| m.len()),
                            modified: meta.as_ref().and_then(|m| m.modified().ok()),
                            hidden: false,
                        };
                        found.push(e.clone());
                        pending.push(e);
                    }
                    if pending.len() >= 128 || scanned % 2000 == 0 {
                        let chunk = SearchChunk {
                            found: std::mem::take(&mut pending),
                            scanned,
                            done: false,
                            truncated,
                        };
                        if tx.send(Msg::Search(chunk)).is_err() {
                            break 'walk;
                        }
                    }
                }

                let elapsed_ms = started.elapsed().as_millis();
                let _ = tx.send(Msg::Search(SearchChunk {
                    found: pending,
                    scanned,
                    done: true,
                    truncated,
                }));
                log::debug!("search: {scanned} scanned in {elapsed_ms} ms");
            })
            .expect("spawn search thread");
    }
}

/// Folders we never descend into: noise plus VCS noise.
fn skip_entry(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.is_empty() {
        return true;
    }
    matches!(
        name.as_str(),
        ".git"
            | ".svn"
            | ".hg"
            | "node_modules"
            | "target"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".cache"
            | ".idea"
            | ".vscode"
            | "dist"
            | "build"
            | ".gradle"
            | ".cargo"
            | "vendor"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_all_terms_anywhere() {
        assert!(matches("Cargo.toml", "cargo"));
        assert!(matches("readme-notes.md", "notes md"));
        assert!(!matches("main.rs", "readme"));
        assert!(matches("Anything", ""));
    }

    #[test]
    fn skip_list_excludes_noise_dirs() {
        assert!(skip_entry(
            std::path::Path::new("/x/.git"),
            std::path::Path::new("/x")
        ));
        assert!(!skip_entry(
            std::path::Path::new("/x/src"),
            std::path::Path::new("/x")
        ));
    }

    #[test]
    fn filter_is_case_insensitive_and_term_based() {
        assert!(matches("Notes.MD", "notes md"));
        assert!(matches("notes", "NOTES"));
        assert!(!matches("notes", "notes other"));
    }
}
