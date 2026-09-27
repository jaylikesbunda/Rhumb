//! The Explorer-style navigation tree.
//!
//! Folders are loaded lazily on a background thread and only when expanded, so
//! the sidebar stays instant no matter how deep the disk goes. Visible rows are
//! recomputed from the node map, which keeps expansion state tiny and makes
//! navigation a single lookup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// State for one folder in the tree.
#[derive(Default, Clone)]
struct Node {
    /// Immediate subfolders, filled in when the node is first expanded.
    children: Vec<PathBuf>,
    expanded: bool,
    loaded: bool,
}

/// The navigation tree.
#[derive(Default)]
pub struct Tree {
    nodes: HashMap<PathBuf, Node>,
    /// Roots, in display order: quick access first, then drives.
    roots: Vec<PathBuf>,
    /// Flattened, visible rows, rebuilt after every change.
    rows: Vec<Row>,
    /// Folder whose children are still being read, if any.
    pending: Option<PathBuf>,
}

/// One visible line in the sidebar.
#[derive(Clone, PartialEq, Debug)]
pub struct Row {
    pub path: PathBuf,
    /// Empty for root entries, which carry their own label.
    pub label: String,
    pub depth: usize,
    pub expanded: bool,
    pub is_root: bool,
}

impl Tree {
    /// Rebuilds the tree from a set of roots, keeping expansion state.
    pub fn set_roots(&mut self, roots: &[PathBuf]) {
        let known: Vec<PathBuf> = roots.to_vec();
        self.roots = known;
        self.rebuild();
    }

    /// Expands a root so its children appear, returning the folder to read.
    pub fn expand_root(&mut self, path: &Path) -> Option<PathBuf> {
        let needs = {
            let node = self.nodes.entry(path.to_path_buf()).or_default();
            if node.expanded {
                return None;
            }
            node.expanded = true;
            !node.loaded
        };
        self.rebuild();
        needs.then(|| path.to_path_buf())
    }

    /// Collapses a root.
    pub fn collapse_root(&mut self, path: &Path) {
        if let Some(node) = self.nodes.get_mut(path) {
            node.expanded = false;
        }
        self.rebuild();
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.nodes.get(path).is_some_and(|n| n.expanded)
    }

    /// Toggles any folder, not just roots.
    #[allow(dead_code)]
    pub fn toggle(&mut self, path: &Path) -> Option<PathBuf> {
        let needs = {
            let node = self.nodes.entry(path.to_path_buf()).or_default();
            if node.expanded {
                node.expanded = false;
                None
            } else {
                node.expanded = true;
                if node.loaded {
                    None
                } else {
                    Some(path.to_path_buf())
                }
            }
        };
        self.rebuild();
        needs
    }

    /// Fills a folder's children after the background read finishes.
    pub fn set_children(&mut self, path: &Path, children: Vec<PathBuf>) {
        if self.pending.as_deref() == Some(path) {
            self.pending = None;
        }
        let node = self.nodes.entry(path.to_path_buf()).or_default();
        node.children = children;
        node.loaded = true;
        self.rebuild();
    }

    /// Marks a folder as being read, so a slow disk shows nothing rather than
    /// an empty list that looks like "no subfolders".
    pub fn set_pending(&mut self, path: PathBuf) {
        self.pending = Some(path);
    }

    pub fn is_pending(&self, path: &Path) -> bool {
        self.pending.as_deref() == Some(path)
    }

    /// Makes sure `path` and every ancestor is expanded, so the folder is
    /// visible in the tree. Only used for roots.
    pub fn reveal(&mut self, path: &PathBuf) {
        if !self.roots.contains(path) {
            return;
        }
        if let Some(node) = self.nodes.get_mut(path) {
            node.expanded = true;
        }
        self.rebuild();
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Rebuilds the flattened, visible row list.
    fn rebuild(&mut self) {
        let mut rows = Vec::with_capacity(self.nodes.len().min(256));
        for root in self.roots.clone() {
            let expanded = self.is_expanded(&root);
            rows.push(Row {
                label: String::new(),
                path: root.clone(),
                depth: 0,
                expanded,
                is_root: true,
            });
            if expanded {
                self.collect_children(&root, 1, &mut rows);
            }
        }
        self.rows = rows;
    }

    fn collect_children(&self, path: &PathBuf, depth: usize, rows: &mut Vec<Row>) {
        let Some(node) = self.nodes.get(path) else {
            return;
        };
        if !node.loaded || depth > MAX_DEPTH {
            return;
        }
        for child in &node.children {
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let expanded = self.is_expanded(child);
            rows.push(Row {
                path: child.clone(),
                label: name,
                depth,
                expanded,
                is_root: false,
            });
            if expanded {
                self.collect_children(child, depth + 1, rows);
            }
        }
    }
}

/// Depth limit for tree expansion.
const MAX_DEPTH: usize = 12;

/// Reads the immediate subfolders of `path` for the tree.
pub fn read_dirs(path: &std::path::Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        out.push(entry.path());
    }
    out.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn roots_are_always_visible() {
        let mut tree = Tree::default();
        tree.set_roots(&[p("/home"), p("/media")]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].is_root && rows[1].is_root);
        assert!(!rows[0].expanded);
    }

    #[test]
    fn expanding_reveals_children_after_they_load() {
        let mut tree = Tree::default();
        tree.set_roots(&[p("/home")]);
        let need = tree.expand_root(&p("/home"));
        assert_eq!(need, Some(p("/home")), "first expand asks for a read");
        // Still nothing until the read lands.
        assert_eq!(tree.rows().len(), 1);

        tree.set_children(&p("/home"), vec![p("/home/docs"), p("/home/pics")]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].label, "docs");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[2].label, "pics");
    }

    #[test]
    fn collapse_hides_children() {
        let mut tree = Tree::default();
        tree.set_roots(&[p("/home")]);
        tree.expand_root(&p("/home"));
        tree.set_children(&p("/home"), vec![p("/home/docs")]);
        assert_eq!(tree.rows().len(), 2);
        tree.collapse_root(&p("/home"));
        assert_eq!(tree.rows().len(), 1);
    }

    #[test]
    fn state_survives_a_rebuild_of_the_root_list() {
        let mut tree = Tree::default();
        tree.set_roots(&[p("/home"), p("/media")]);
        tree.expand_root(&p("/media"));
        tree.set_children(&p("/media"), vec![p("/media/fotos")]);
        // Two roots plus one child.
        assert_eq!(tree.rows().len(), 3);
        // A later refresh (the sidebar is rebuilt every frame) keeps the state.
        tree.set_roots(&[p("/home"), p("/media")]);
        assert_eq!(tree.rows().len(), 3);
        assert!(tree.is_expanded(&p("/media")));
    }

    #[test]
    fn nested_expansion_nests_indent() {
        let mut tree = Tree::default();
        tree.set_roots(&[p("/home")]);
        tree.expand_root(&p("/home"));
        tree.set_children(&p("/home"), vec![p("/home/docs")]);
        let need = tree.toggle(&p("/home/docs"));
        assert_eq!(need, Some(p("/home/docs")));
        tree.set_children(&p("/home/docs"), vec![p("/home/docs/2026")]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[2].label, "2026");
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    /// The sidebar paints exactly the rows the tree hands it, in order. A
    /// regression here is invisible in the tree itself and very visible on
    /// screen: a folder's children appear at the bottom of the list instead of
    /// under their parent.
    #[test]
    fn a_child_comes_immediately_after_its_parent() {
        let mut tree = Tree::default();
        tree.set_roots(&[PathBuf::from("/home"), PathBuf::from("/media")]);
        tree.expand_root(Path::new("/home"));
        tree.set_children(
            Path::new("/home"),
            vec![PathBuf::from("/home/docs"), PathBuf::from("/home/pics")],
        );
        let order: Vec<(PathBuf, bool)> = tree
            .rows()
            .iter()
            .map(|r| (r.path.clone(), r.is_root))
            .collect();
        assert_eq!(
            order,
            vec![
                (PathBuf::from("/home"), true),
                (PathBuf::from("/home/docs"), false),
                (PathBuf::from("/home/pics"), false),
                (PathBuf::from("/media"), true),
            ],
            "roots and descendants must be interleaved in tree order"
        );
    }

    #[test]
    fn roots_come_back_when_a_folder_is_collapsed() {
        let mut tree = Tree::default();
        tree.set_roots(&[PathBuf::from("/home"), PathBuf::from("/media")]);
        tree.expand_root(Path::new("/media"));
        tree.set_children(Path::new("/media"), vec![PathBuf::from("/media/x")]);
        assert_eq!(tree.rows().len(), 3);
        tree.collapse_root(Path::new("/media"));
        assert_eq!(tree.rows().len(), 2);
        assert!(tree.rows().iter().all(|r| r.is_root));
    }

    #[test]
    fn nested_folders_unwind_in_the_right_order() {
        let mut tree = Tree::default();
        tree.set_roots(&[PathBuf::from("/a"), PathBuf::from("/b")]);
        tree.expand_root(Path::new("/a"));
        tree.set_children(Path::new("/a"), vec![PathBuf::from("/a/x")]);
        tree.expand_root(Path::new("/a/x"));
        tree.set_children(Path::new("/a/x"), vec![PathBuf::from("/a/x/deep")]);
        let paths: Vec<&Path> = tree.rows().iter().map(|r| r.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                Path::new("/a"),
                Path::new("/a/x"),
                Path::new("/a/x/deep"),
                Path::new("/b"),
            ]
        );
        // Depths line up with how deeply nested each one is.
        let depths: Vec<usize> = tree.rows().iter().map(|r| r.depth).collect();
        assert_eq!(depths, vec![0, 1, 2, 0]);
    }
}
