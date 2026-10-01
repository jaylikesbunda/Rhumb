//! A name index of a folder tree, for searches that answer at once.
//!
//! Walking the disk for every search is as slow as the disk: a tree of a few hundred
//! thousand files takes seconds, and the search has to start again whenever a letter
//! is typed. An index walks it once, on a thread of its own, and keeps the names in
//! memory; a search is then a scan of a vector, which is a few milliseconds for a
//! million names. Matches are ranked, so the file that was meant comes first rather
//! than whichever the walk met first.
//!
//! The index is kept up to date by being told which folder changed
//! ([`Index::rescan_dir`]), not by walking again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime};

use crate::fs_model::Entry;

/// The most names one index holds. A tree bigger than this is indexed as far as it
/// goes and then marked truncated, so memory stays bounded.
pub const MAX_ENTRIES: usize = 2_000_000;
/// How deep a walk goes.
pub const MAX_DEPTH: usize = 32;
/// How many indexes are kept at once; the one used least recently makes room.
pub const MAX_INDEXES: usize = 4;

/// One name in the index.
#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub path: PathBuf,
    /// The name, as shown.
    pub name: String,
    /// The name in lower case, which is what is matched against.
    pub lower: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// How many folders down from the root, for ranking.
    pub depth: u16,
}

/// What the build thread is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Building,
    Ready,
}

struct Inner {
    entries: Vec<IndexEntry>,
    state: State,
    truncated: bool,
    built_at: Instant,
}

/// Stops the build thread when the last handle to the index is dropped.
struct Guard(Arc<AtomicBool>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A handle on one index. Cloning it is cheap and every clone sees the same names;
/// the build thread is stopped when the last of them goes.
#[derive(Clone)]
pub struct Index {
    pub root: PathBuf,
    inner: Arc<RwLock<Inner>>,
    scanned: Arc<AtomicU64>,
    _guard: Arc<Guard>,
}

impl Index {
    /// Starts indexing `root` on a thread of its own and returns at once. The index
    /// can be searched straight away, and then holds what has been found so far.
    pub fn build(root: &Path) -> Index {
        let cancel = Arc::new(AtomicBool::new(false));
        let index = Index {
            root: root.to_path_buf(),
            inner: Arc::new(RwLock::new(Inner {
                entries: Vec::new(),
                state: State::Building,
                truncated: false,
                built_at: Instant::now(),
            })),
            scanned: Arc::new(AtomicU64::new(0)),
            _guard: Arc::new(Guard(cancel.clone())),
        };
        let inner = index.inner.clone();
        let scanned = index.scanned.clone();
        let root = root.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("rhumb-index".into())
            .spawn(move || {
                let mut batch: Vec<IndexEntry> = Vec::with_capacity(2048);
                let mut truncated = false;
                let mut total = 0usize;
                for entry in walkdir::WalkDir::new(&root)
                    .max_depth(MAX_DEPTH)
                    .follow_links(false)
                    .into_iter()
                    .filter_entry(|e| !crate::search::skip_entry(e.path(), &root))
                    .filter_map(Result::ok)
                {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    if entry.path() == root {
                        continue;
                    }
                    if total >= MAX_ENTRIES {
                        truncated = true;
                        break;
                    }
                    total += 1;
                    batch.push(make_entry(&entry));
                    if batch.len() >= 2048 {
                        scanned.store(total as u64, Ordering::Relaxed);
                        let mut w = inner.write().unwrap_or_else(|e| e.into_inner());
                        w.entries.append(&mut batch);
                    }
                }
                scanned.store(total as u64, Ordering::Relaxed);
                let mut w = inner.write().unwrap_or_else(|e| e.into_inner());
                w.entries.append(&mut batch);
                w.truncated = truncated;
                w.built_at = Instant::now();
                w.state = State::Ready;
            });
        if spawned.is_err() {
            // No thread to build on: an empty index that says so, rather than one that
            // claims to be filling for ever.
            let mut w = index.inner.write().unwrap_or_else(|e| e.into_inner());
            w.state = State::Ready;
        }
        index
    }

    pub fn state(&self) -> State {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).state
    }

    pub fn is_ready(&self) -> bool {
        self.state() == State::Ready
    }

    /// How many names are held so far.
    pub fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }

    /// Whether the walk stopped at the size limit.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn truncated(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .truncated
    }

    /// How many items the build has gone through, which is shown while it runs.
    pub fn scanned(&self) -> u64 {
        self.scanned.load(Ordering::Relaxed)
    }

    /// Whether `dir` is this index's root or inside it.
    pub fn covers(&self, dir: &Path) -> bool {
        dir.starts_with(&self.root)
    }

    /// Everything under `scope` whose name matches every word of `query`, best first,
    /// and whether there were more than `limit`.
    pub fn search(&self, scope: &Path, query: &str, limit: usize) -> (Vec<Entry>, bool) {
        let terms = crate::search::prepare(query);
        if terms.is_empty() {
            return (Vec::new(), false);
        }
        let whole = terms.join(" ");
        let r = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut hits: Vec<(u32, u16, usize, usize)> = Vec::new();
        for (i, e) in r.entries.iter().enumerate() {
            if !terms.iter().all(|t| e.lower.contains(t.as_str())) {
                continue;
            }
            if scope != self.root && !e.path.starts_with(scope) {
                continue;
            }
            hits.push((
                rank(&e.lower, &terms, &whole),
                e.depth,
                e.lower.chars().count(),
                i,
            ));
        }
        let truncated = hits.len() > limit;
        // Best first: the match that fits the query most closely, then the one nearest
        // the top of the tree, then the shorter name, then the order they were found.
        if truncated {
            hits.select_nth_unstable(limit);
            hits.truncate(limit);
        }
        hits.sort_unstable();
        let found = hits
            .into_iter()
            .map(|(_, _, _, i)| {
                let e = &r.entries[i];
                Entry {
                    name: e.name.clone(),
                    path: e.path.clone(),
                    is_dir: e.is_dir,
                    is_symlink: false,
                    size: e.size,
                    modified: e.modified,
                    hidden: false,
                }
            })
            .collect();
        (found, truncated)
    }

    /// Brings the index up to date with one folder that has changed: names that are
    /// gone are dropped (with everything under them), names that are new are added
    /// (with everything under them), and the ones that stay are refreshed.
    pub fn rescan_dir(&self, dir: &Path) {
        if !self.covers(dir) {
            return;
        }
        let fresh: Vec<walkdir::DirEntry> = match std::fs::read_dir(dir) {
            Ok(_) => walkdir::WalkDir::new(dir)
                .min_depth(1)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| !crate::search::skip_entry(e.path(), &self.root))
                .filter_map(Result::ok)
                .collect(),
            // The folder itself is gone: everything under it goes.
            Err(_) => {
                self.remove_under(dir);
                return;
            }
        };
        let depth_of = |p: &Path| -> u16 {
            p.strip_prefix(&self.root)
                .map_or(0, |r| r.components().count().saturating_sub(1) as u16)
        };
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let present: std::collections::HashSet<PathBuf> =
            fresh.iter().map(|e| e.path().to_path_buf()).collect();
        // Drop the children that are no longer there, and what was under them.
        w.entries.retain(|e| {
            let is_child = e.path.parent() == Some(dir);
            let under_child = e.path.starts_with(dir) && e.path != *dir;
            if !under_child {
                return true;
            }
            if is_child {
                return present.contains(&e.path);
            }
            // A grandchild stays while the child it hangs from does.
            let child = e
                .path
                .ancestors()
                .find(|a| a.parent() == Some(dir))
                .map(Path::to_path_buf);
            child.is_none_or(|c| present.contains(&c))
        });
        let known: std::collections::HashSet<PathBuf> = w
            .entries
            .iter()
            .filter(|e| e.path.parent() == Some(dir))
            .map(|e| e.path.clone())
            .collect();
        for e in &fresh {
            let path = e.path();
            if known.contains(path) {
                // Still there: its size and date may have changed.
                if let Some(slot) = w.entries.iter_mut().find(|x| x.path == path) {
                    let meta = e.metadata().ok();
                    slot.size = meta.as_ref().map_or(0, |m| m.len());
                    slot.modified = meta.as_ref().and_then(|m| m.modified().ok());
                }
                continue;
            }
            if w.entries.len() >= MAX_ENTRIES {
                break;
            }
            let mut me = make_entry(e);
            me.depth = depth_of(path);
            let is_dir = me.is_dir;
            w.entries.push(me);
            if is_dir {
                // A folder that is new comes with what is in it.
                for sub in walkdir::WalkDir::new(path)
                    .min_depth(1)
                    .max_depth(MAX_DEPTH)
                    .follow_links(false)
                    .into_iter()
                    .filter_entry(|x| !crate::search::skip_entry(x.path(), &self.root))
                    .filter_map(Result::ok)
                {
                    if w.entries.len() >= MAX_ENTRIES {
                        break;
                    }
                    let mut se = make_entry(&sub);
                    se.depth = depth_of(sub.path());
                    w.entries.push(se);
                }
            }
        }
        w.built_at = Instant::now();
    }

    /// Forgets a path and everything under it.
    pub fn remove_under(&self, path: &Path) {
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        w.entries.retain(|e| !e.path.starts_with(path));
    }
}

fn make_entry(entry: &walkdir::DirEntry) -> IndexEntry {
    let meta = entry.metadata().ok();
    let name = entry.file_name().to_string_lossy().into_owned();
    IndexEntry {
        path: entry.path().to_path_buf(),
        lower: name.to_lowercase(),
        name,
        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
        size: meta.as_ref().map_or(0, |m| m.len()),
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        depth: entry.depth().saturating_sub(1) as u16,
    }
}

/// How well a name fits the query, the lowest number being the best.
///
/// The whole query as the whole name is best, then a name that starts with the first
/// word, then one that has a word starting where a word of the name starts, and last
/// the words found somewhere in the middle of it.
pub fn rank(lower: &str, terms: &[String], whole: &str) -> u32 {
    if lower == whole {
        return 0;
    }
    // The name without its extension, for `report` against `report.txt`.
    let stem = lower.rsplit_once('.').map_or(lower, |(s, _)| s);
    if stem == whole {
        return 1;
    }
    let first = terms[0].as_str();
    if lower.starts_with(first) {
        return 2;
    }
    let at_boundary = |t: &str| {
        lower.match_indices(t).any(|(i, _)| {
            i == 0
                || lower[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_alphanumeric())
        })
    };
    if terms.iter().all(|t| at_boundary(t)) {
        return 3;
    }
    4
}

/// The indexes a window keeps: one per tree searched, the least recently used given
/// up when there are too many.
#[derive(Default)]
pub struct Indexes {
    list: Vec<Index>,
}

impl Indexes {
    /// The index that covers `dir`, starting one rooted at `dir` if none does.
    pub fn ensure(&mut self, dir: &Path) -> &Index {
        if let Some(i) = self.list.iter().position(|x| x.covers(dir)) {
            // The newest is kept at the end.
            let found = self.list.remove(i);
            self.list.push(found);
        } else {
            // A new, wider one makes any it covers redundant.
            self.list.retain(|x| !x.root.starts_with(dir));
            self.list.push(Index::build(dir));
            while self.list.len() > MAX_INDEXES {
                self.list.remove(0);
            }
        }
        self.list.last().expect("just pushed")
    }

    /// The index for `dir` if there is one that has finished.
    pub fn ready_for(&self, dir: &Path) -> Option<&Index> {
        self.list.iter().find(|x| x.covers(dir) && x.is_ready())
    }

    /// An index covering `dir`, finished or not.
    pub fn any_for(&self, dir: &Path) -> Option<&Index> {
        self.list.iter().find(|x| x.covers(dir))
    }

    /// Something changed in `dir`: every index that holds it is brought up to date.
    pub fn changed(&self, dir: &Path) {
        for x in &self.list {
            if x.covers(dir) {
                let x = x.clone();
                let dir = dir.to_path_buf();
                let _ = std::thread::Builder::new()
                    .name("rhumb-reindex".into())
                    .spawn(move || x.rescan_dir(&dir));
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn roots(&self) -> Vec<PathBuf> {
        self.list.iter().map(|x| x.root.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rhumb-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
        std::fs::write(root.join("Cargo.toml"), b"[package]").unwrap();
        std::fs::write(root.join("README.md"), b"hello").unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("src/lib.rs"), b"").unwrap();
        std::fs::write(root.join("src/deep/notes.md"), b"").unwrap();
        std::fs::write(root.join("src/deep/deeper/readme-deep.md"), b"").unwrap();
        // Noise that is never indexed.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config.rs"), b"").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.rs"), b"").unwrap();
        root
    }

    fn wait(ix: &Index) {
        for _ in 0..2000 {
            if ix.is_ready() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the index never finished");
    }

    fn names(found: &[Entry]) -> Vec<String> {
        found.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn an_index_fills_on_its_own_thread_and_then_says_it_is_ready() {
        let root = tree("ready");
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(ix.state(), State::Ready);
        // 6 files + src, src/deep, src/deep/deeper.
        assert_eq!(ix.len(), 9, "{:?}", ix.search(&root, "e", 100).0);
        assert!(!ix.truncated());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn noise_folders_are_not_indexed() {
        let root = tree("noise");
        let ix = Index::build(&root);
        wait(&ix);
        let (all, _) = ix.search(&root, "rs", 100);
        let n = names(&all);
        assert!(n.contains(&"main.rs".to_owned()));
        assert!(!n.contains(&"config.rs".to_owned()), "{n:?}");
        assert!(!n.contains(&"index.rs".to_owned()), "{n:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn every_word_must_be_in_the_name_in_any_order_and_case() {
        let root = tree("terms");
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(names(&ix.search(&root, "NOTES md", 10).0), vec!["notes.md"]);
        assert_eq!(names(&ix.search(&root, "md notes", 10).0), vec!["notes.md"]);
        assert!(ix.search(&root, "notes zzz", 10).0.is_empty());
        assert_eq!(ix.search(&root, "readme", 10).0.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_blank_query_matches_nothing() {
        let root = tree("blank");
        let ix = Index::build(&root);
        wait(&ix);
        assert!(ix.search(&root, "", 10).0.is_empty());
        assert!(ix.search(&root, "   \t ", 10).0.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn matches_are_ranked_exact_then_prefix_then_the_rest() {
        let root = std::env::temp_dir().join(format!("rhumb-index-rank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/report"), b"").unwrap();
        std::fs::write(root.join("report.txt"), b"").unwrap();
        std::fs::write(root.join("a/annual-report.txt"), b"").unwrap();
        std::fs::write(root.join("reports-old.txt"), b"").unwrap();
        std::fs::write(root.join("xreportx.txt"), b"").unwrap();
        let ix = Index::build(&root);
        wait(&ix);
        let found = names(&ix.search(&root, "report", 10).0);
        // The name that is exactly the query, then the one that is it with an
        // extension, then one that starts with it, then a word of it, then inside.
        assert_eq!(
            found,
            vec![
                "report",
                "report.txt",
                "reports-old.txt",
                "annual-report.txt",
                "xreportx.txt"
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn of_two_equal_matches_the_one_nearer_the_top_comes_first() {
        let root = std::env::temp_dir().join(format!("rhumb-index-depth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("x/y/z")).unwrap();
        std::fs::write(root.join("x/y/z/todo.txt"), b"").unwrap();
        std::fs::write(root.join("x/todo.md"), b"").unwrap();
        std::fs::write(root.join("todo.rs"), b"").unwrap();
        let ix = Index::build(&root);
        wait(&ix);
        let found = names(&ix.search(&root, "todo", 10).0);
        assert_eq!(found, vec!["todo.rs", "todo.md", "todo.txt"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_search_inside_a_subfolder_only_sees_that_subfolder() {
        let root = tree("scope");
        let ix = Index::build(&root);
        wait(&ix);
        let sub = root.join("src/deep");
        let found = names(&ix.search(&sub, "md", 10).0);
        assert_eq!(found, vec!["notes.md", "readme-deep.md"]);
        assert!(ix.covers(&sub));
        assert!(!ix.covers(&root.parent().unwrap().join("elsewhere")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_limit_keeps_the_best_and_says_there_were_more() {
        let root = std::env::temp_dir().join(format!("rhumb-index-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..50 {
            std::fs::write(root.join(format!("file{i:02}.txt")), b"").unwrap();
        }
        std::fs::write(root.join("file.txt"), b"").unwrap();
        let ix = Index::build(&root);
        wait(&ix);
        let (found, more) = ix.search(&root, "file", 5);
        assert_eq!(found.len(), 5);
        assert!(more, "there were 51");
        assert_eq!(found[0].name, "file.txt", "and the best is among them");
        let (all, more) = ix.search(&root, "file", 100);
        assert_eq!(all.len(), 51);
        assert!(!more);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_with_accents_and_capitals_are_found_by_their_lower_case() {
        let root = std::env::temp_dir().join(format!("rhumb-index-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("ÄÖÜ Straße.txt"), b"").unwrap();
        std::fs::write(root.join("日本語.md"), b"").unwrap();
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(ix.search(&root, "äöü", 10).0.len(), 1);
        assert_eq!(ix.search(&root, "STRASSE", 10).0.len(), 0, "ß is not ss");
        assert_eq!(ix.search(&root, "日本", 10).0.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn characters_that_mean_something_to_a_pattern_are_taken_literally() {
        let root = std::env::temp_dir().join(format!("rhumb-index-lit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a+b (1).txt"), b"").unwrap();
        std::fs::write(root.join("a.b.txt"), b"").unwrap();
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(ix.search(&root, "a+b", 10).0.len(), 1);
        assert_eq!(ix.search(&root, "(1)", 10).0.len(), 1);
        assert_eq!(ix.search(&root, ".*", 10).0.len(), 0);
        assert_eq!(ix.search(&root, "a.b", 10).0.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn results_carry_what_the_list_needs_to_show_them() {
        let root = tree("fields");
        let ix = Index::build(&root);
        wait(&ix);
        let (found, _) = ix.search(&root, "main.rs", 10);
        let e = &found[0];
        assert_eq!(e.name, "main.rs");
        assert_eq!(e.path, root.join("src/main.rs"));
        assert!(!e.is_dir);
        assert_eq!(e.size, 12);
        assert!(e.modified.is_some());
        let (dirs, _) = ix.search(&root, "deeper", 10);
        assert!(dirs.iter().any(|d| d.is_dir && d.name == "deeper"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_file_in_a_changed_folder_is_found_after_a_rescan() {
        let root = tree("add");
        let ix = Index::build(&root);
        wait(&ix);
        assert!(ix.search(&root, "fresh", 10).0.is_empty());
        std::fs::write(root.join("src/fresh.rs"), b"x").unwrap();
        ix.rescan_dir(&root.join("src"));
        assert_eq!(names(&ix.search(&root, "fresh", 10).0), vec!["fresh.rs"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_deleted_file_is_gone_after_a_rescan() {
        let root = tree("del");
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(ix.search(&root, "lib.rs", 10).0.len(), 1);
        std::fs::remove_file(root.join("src/lib.rs")).unwrap();
        ix.rescan_dir(&root.join("src"));
        assert!(ix.search(&root, "lib.rs", 10).0.is_empty());
        // And the neighbours are untouched.
        assert_eq!(ix.search(&root, "main.rs", 10).0.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_deleted_folder_takes_everything_under_it_with_it() {
        let root = tree("deldir");
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(ix.search(&root, "readme-deep", 10).0.len(), 1);
        std::fs::remove_dir_all(root.join("src/deep")).unwrap();
        ix.rescan_dir(&root.join("src"));
        assert!(ix.search(&root, "readme-deep", 10).0.is_empty());
        assert!(ix.search(&root, "notes", 10).0.is_empty());
        assert!(ix.search(&root, "deep", 10).0.is_empty());
        assert_eq!(ix.search(&root, "main", 10).0.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_folder_comes_with_what_is_in_it() {
        let root = tree("adddir");
        let ix = Index::build(&root);
        wait(&ix);
        std::fs::create_dir_all(root.join("src/newdir/inner")).unwrap();
        std::fs::write(root.join("src/newdir/inner/buried.txt"), b"").unwrap();
        ix.rescan_dir(&root.join("src"));
        assert_eq!(names(&ix.search(&root, "buried", 10).0), vec!["buried.txt"]);
        let found = ix.search(&root, "newdir", 10).0;
        assert!(found.iter().any(|e| e.is_dir));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rescan_refreshes_the_size_of_a_file_that_stayed() {
        let root = tree("size");
        let ix = Index::build(&root);
        wait(&ix);
        assert_eq!(ix.search(&root, "main.rs", 1).0[0].size, 12);
        std::fs::write(root.join("src/main.rs"), b"fn main() { println!(); }").unwrap();
        ix.rescan_dir(&root.join("src"));
        assert_eq!(ix.search(&root, "main.rs", 1).0[0].size, 25);
        assert_eq!(
            ix.search(&root, "main", 10).0.len(),
            1,
            "and it is not doubled"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescanning_a_folder_that_is_gone_clears_it() {
        let root = tree("gone");
        let ix = Index::build(&root);
        wait(&ix);
        std::fs::remove_dir_all(root.join("src")).unwrap();
        ix.rescan_dir(&root.join("src"));
        assert!(ix.search(&root, "main", 10).0.is_empty());
        assert_eq!(ix.search(&root, "cargo", 10).0.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescanning_somewhere_the_index_does_not_cover_does_nothing() {
        let root = tree("outside");
        let other = tree("outside-other");
        let ix = Index::build(&root);
        wait(&ix);
        let before = ix.len();
        ix.rescan_dir(&other);
        assert_eq!(ix.len(), before);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_search_while_the_index_is_still_filling_is_safe_and_only_ever_grows() {
        let root = std::env::temp_dir().join(format!("rhumb-index-grow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for d in 0..20 {
            std::fs::create_dir_all(root.join(format!("d{d}"))).unwrap();
            for f in 0..60 {
                std::fs::write(root.join(format!("d{d}/f{f}.txt")), b"").unwrap();
            }
        }
        let ix = Index::build(&root);
        let mut last = 0;
        for _ in 0..4000 {
            let n = ix.search(&root, "txt", usize::MAX / 2).0.len();
            assert!(n >= last, "{n} < {last}");
            last = n;
            if ix.is_ready() {
                break;
            }
        }
        wait(&ix);
        assert_eq!(ix.search(&root, "txt", 10_000).0.len(), 1200);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_build_carries_on_while_any_handle_is_held_and_stops_when_the_last_goes() {
        let root = std::env::temp_dir().join(format!("rhumb-index-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for d in 0..30 {
            std::fs::create_dir_all(root.join(format!("d{d}"))).unwrap();
            for f in 0..100 {
                std::fs::write(root.join(format!("d{d}/f{f}.txt")), b"").unwrap();
            }
        }
        let ix = Index::build(&root);
        let probe = ix.inner.clone();
        let clone = ix.clone();
        drop(ix);
        // A clone still holds it, so the build goes on to the end.
        wait(&clone);
        assert_eq!(clone.len(), 3030);
        drop(clone);
        let mut gone = false;
        for _ in 0..1000 {
            if Arc::strong_count(&probe) == 1 {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(gone, "the build thread still holds the index");

        // Dropped at once, before it can finish, the thread leaves early and lets go.
        let ix = Index::build(&root);
        let probe = ix.inner.clone();
        drop(ix);
        let mut gone = false;
        for _ in 0..1000 {
            if Arc::strong_count(&probe) == 1 {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(gone, "an abandoned build kept running");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_index_of_a_folder_that_does_not_exist_is_ready_and_empty() {
        let ix = Index::build(Path::new("no-such-folder-anywhere-rhumb"));
        wait(&ix);
        assert_eq!(ix.len(), 0);
        assert!(
            ix.search(Path::new("no-such-folder-anywhere-rhumb"), "a", 5)
                .0
                .is_empty()
        );
    }

    #[test]
    fn the_depth_of_a_name_counts_the_folders_above_it() {
        let root = tree("depth");
        let ix = Index::build(&root);
        wait(&ix);
        let r = ix.inner.read().unwrap();
        let depth = |n: &str| r.entries.iter().find(|e| e.name == n).unwrap().depth;
        assert_eq!(depth("Cargo.toml"), 0);
        assert_eq!(depth("main.rs"), 1);
        assert_eq!(depth("notes.md"), 2);
        assert_eq!(depth("readme-deep.md"), 3);
        drop(r);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- the set of indexes ---------------------------------------------------------

    #[test]
    fn asking_for_a_folder_starts_an_index_and_asking_inside_it_reuses_it() {
        let root = tree("set-reuse");
        let mut set = Indexes::default();
        set.ensure(&root);
        assert_eq!(set.len(), 1);
        set.ensure(&root.join("src"));
        set.ensure(&root.join("src/deep"));
        assert_eq!(set.len(), 1, "one index serves them all");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_wider_folder_replaces_the_narrow_indexes_it_covers() {
        let root = tree("set-wider");
        let mut set = Indexes::default();
        set.ensure(&root.join("src"));
        set.ensure(&root.join(".git"));
        assert_eq!(set.len(), 2);
        set.ensure(&root);
        assert_eq!(set.len(), 1);
        assert_eq!(set.roots(), vec![root.clone()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_so_many_indexes_are_kept_and_the_oldest_goes_first() {
        let base = std::env::temp_dir().join(format!("rhumb-index-lru-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mut set = Indexes::default();
        let mut dirs = Vec::new();
        for i in 0..MAX_INDEXES + 2 {
            let d = base.join(format!("t{i}"));
            std::fs::create_dir_all(&d).unwrap();
            dirs.push(d);
        }
        for d in &dirs {
            set.ensure(d);
        }
        assert_eq!(set.len(), MAX_INDEXES);
        assert!(set.any_for(&dirs[0]).is_none(), "the oldest is gone");
        assert!(set.any_for(dirs.last().unwrap()).is_some());
        // Using one makes it the newest, so it survives the next arrival.
        set.ensure(&dirs[2]);
        let extra = base.join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        set.ensure(&extra);
        assert!(set.any_for(&dirs[2]).is_some());
        assert!(set.any_for(&dirs[3]).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_folder_is_only_searched_by_an_index_that_has_finished() {
        let root = tree("set-ready");
        let mut set = Indexes::default();
        assert!(set.is_empty());
        assert!(set.ready_for(&root).is_none());
        let ix = set.ensure(&root).clone();
        wait(&ix);
        assert!(set.ready_for(&root).is_some());
        assert!(set.ready_for(&root.join("src")).is_some());
        assert!(
            set.ready_for(&root.parent().unwrap().join("nowhere"))
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn telling_the_set_a_folder_changed_updates_the_index_that_holds_it() {
        let root = tree("set-changed");
        let mut set = Indexes::default();
        let ix = set.ensure(&root).clone();
        wait(&ix);
        std::fs::write(root.join("src/appeared.rs"), b"").unwrap();
        set.changed(&root.join("src"));
        let mut seen = false;
        for _ in 0..1000 {
            if !ix.search(&root, "appeared", 5).0.is_empty() {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        assert!(seen, "the new file never reached the index");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_orders_the_five_kinds_of_match() {
        let t = |s: &str| vec![s.to_owned()];
        assert_eq!(rank("report", &t("report"), "report"), 0);
        assert_eq!(rank("report.txt", &t("report"), "report"), 1);
        assert_eq!(rank("reports.txt", &t("report"), "report"), 2);
        assert_eq!(rank("my-report.txt", &t("report"), "report"), 3);
        assert_eq!(rank("myreport.txt", &t("report"), "report"), 4);
    }
}
