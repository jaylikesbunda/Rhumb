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
    /// Counts the searches started, and goes on every chunk a search sends.
    pub token: u64,
    /// Whether the results came from an index and not from a walk of the disk.
    pub indexed: bool,
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

    /// Starts a search that asks an index instead of the disk: the answer is the best
    /// matches first, and arrives as soon as the scan of the names is done.
    pub fn start_indexed(
        &mut self,
        index: crate::index::Index,
        scope: &Path,
        query: &str,
        tx: Sender<Msg>,
    ) {
        self.cancel();
        self.token += 1;
        let token = self.token;
        self.query = query.to_owned();
        self.results.clear();
        self.scanned = 0;
        self.truncated = false;
        if prepare(query).is_empty() {
            self.running = false;
            self.indexed = false;
            return;
        }
        self.running = true;
        self.indexed = true;
        let (scope, query) = (scope.to_path_buf(), query.to_owned());
        let spawned = std::thread::Builder::new()
            .name("rhumb-index-search".into())
            .spawn(move || {
                let (found, truncated) = index.search(&scope, &query, MAX_RESULTS);
                let _ = tx.send(Msg::Search(SearchChunk {
                    token,
                    found,
                    scanned: index.len() as u64,
                    done: true,
                    truncated,
                }));
            });
        if spawned.is_err() {
            self.running = false;
        }
    }

    /// Starts (or restarts) a search rooted at `root`.
    pub fn start(&mut self, root: &Path, query: &str, tx: Sender<Msg>) {
        self.cancel();
        self.token += 1;
        let token = self.token;
        self.indexed = false;
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

        let spawned = std::thread::Builder::new()
            .name("rhumb-search".into())
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
                    if pending.len() >= 128 || scanned.is_multiple_of(2000) {
                        let chunk = SearchChunk {
                            token,
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
                    token,
                    found: pending,
                    scanned,
                    done: true,
                    truncated,
                }));
                log::debug!("search: {scanned} scanned in {elapsed_ms} ms");
            });
        if spawned.is_err() {
            // The walk never started, so nothing is left waiting on it.
            self.running = false;
            log::error!("could not start the search worker");
        }
    }
}

/// Folders we never descend into: noise plus VCS noise.
pub fn skip_entry(path: &Path, root: &Path) -> bool {
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
    use std::path::PathBuf;

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

    /// Builds a small tree and returns its root.
    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rhumb-search-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("Cargo.toml"), b"").unwrap();
        std::fs::write(root.join("src/main.rs"), b"").unwrap();
        std::fs::write(root.join("src/deep/notes.md"), b"").unwrap();
        // Noise that must never be searched.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/hidden.rs"), b"").unwrap();
        root
    }

    /// Runs a search to completion and collects the names it reported.
    fn run(root: &Path, query: &str) -> Vec<String> {
        let (tx, rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start(root, query, tx);
        let mut names = Vec::new();
        for _ in 0..600 {
            match rx.try_recv() {
                Ok(Msg::Search(chunk)) => {
                    for e in &chunk.found {
                        names.push(e.name.clone());
                    }
                    if chunk.done {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        names.sort();
        names
    }

    #[test]
    fn the_walk_finds_files_in_subfolders_and_skips_noise() {
        let root = tree("walk");
        let found = run(&root, "rs");
        assert!(found.contains(&"main.rs".to_owned()), "{found:?}");
        // The .git directory is skipped, so its contents never appear.
        assert!(!found.contains(&"hidden.rs".to_owned()), "{found:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_term_must_match_somewhere_in_the_name() {
        let root = tree("terms");
        // "notes md" only matches the nested markdown file.
        assert_eq!(run(&root, "notes md"), vec!["notes.md".to_owned()]);
        assert!(run(&root, "cargo zzz").is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_query_never_starts_a_walk() {
        let root = tree("empty");
        let (tx, _rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start(&root, "   ", tx);
        assert!(!s.running, "a blank query must not walk the disk");
        assert!(s.results.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_root_is_a_no_op() {
        let (tx, _rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start(Path::new("no-such-folder-anywhere"), "x", tx);
        assert!(!s.running);
    }

    #[test]
    fn cancelling_stops_the_worker() {
        let root = tree("cancel");
        let (tx, rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start(&root, "rs", tx);
        assert!(s.running);
        s.cancel();
        assert!(!s.running);
        // Draining what already arrived must not panic or block.
        while rx.try_recv().is_ok() {}
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- searches answered from an index -----------------------------------------------

    fn index_of(root: &Path) -> crate::index::Index {
        let ix = crate::index::Index::build(root);
        for _ in 0..2000 {
            if ix.is_ready() {
                return ix;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the index never finished");
    }

    fn collect(rx: &std::sync::mpsc::Receiver<Msg>) -> (Vec<String>, u64, bool) {
        for _ in 0..2000 {
            match rx.try_recv() {
                Ok(Msg::Search(c)) if c.done => {
                    return (
                        c.found.iter().map(|e| e.name.clone()).collect(),
                        c.token,
                        c.truncated,
                    );
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        }
        panic!("no answer arrived");
    }

    #[test]
    fn an_indexed_search_answers_with_the_best_match_first() {
        let root = tree("idx-rank");
        std::fs::write(root.join("notes.md.bak"), b"").unwrap();
        let ix = index_of(&root);
        let (tx, rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start_indexed(ix, &root, "notes", tx);
        assert!(s.running && s.indexed);
        let (names, token, truncated) = collect(&rx);
        assert_eq!(names, vec!["notes.md", "notes.md.bak"]);
        assert_eq!(token, s.token);
        assert!(!truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_search_has_a_token_of_its_own() {
        let root = tree("idx-token");
        let ix = index_of(&root);
        let (tx, rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start_indexed(ix.clone(), &root, "rs", tx.clone());
        let first = s.token;
        s.start_indexed(ix, &root, "md", tx);
        assert!(s.token > first);
        // Both answers arrive, tagged, so the older one can be told from the current one.
        let (_, a, _) = collect(&rx);
        let (_, b, _) = collect(&rx);
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_indexed_search_of_a_blank_query_never_starts() {
        let root = tree("idx-blank");
        let ix = index_of(&root);
        let (tx, _rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start_indexed(ix, &root, "  ", tx);
        assert!(!s.running);
        assert!(!s.indexed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_indexed_search_stays_inside_the_folder_it_was_asked_about() {
        let root = tree("idx-scope");
        let ix = index_of(&root);
        let (tx, rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start_indexed(ix, &root.join("src/deep"), "md", tx);
        let (names, _, _) = collect(&rx);
        assert_eq!(names, vec!["notes.md"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plain_walk_after_an_indexed_search_is_not_marked_as_indexed() {
        let root = tree("idx-mode");
        let ix = index_of(&root);
        let (tx, _rx) = crate::workers::bus();
        let mut s = Search::default();
        s.start_indexed(ix, &root, "rs", tx.clone());
        assert!(s.indexed);
        s.start(&root, "rs", tx);
        assert!(!s.indexed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walked_and_indexed_searches_find_the_same_files() {
        let root = tree("idx-same");
        let ix = index_of(&root);
        for q in ["rs", "md", "readme", "main rs", "e"] {
            let walked = run(&root, q);
            let (tx, rx) = crate::workers::bus();
            let mut s = Search::default();
            s.start_indexed(ix.clone(), &root, q, tx);
            let (mut indexed, _, _) = collect(&rx);
            indexed.sort();
            assert_eq!(indexed, walked, "query {q:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
