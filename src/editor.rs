//! Document buffers: loading, dirty tracking, saving and change detection.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui_code_editor::Syntax;

use crate::fs_model::{self, MAX_EDIT_BYTES, is_markdown};

/// What the app does with an opened file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    /// Rendered as Markdown next to the editor.
    Markdown,
    /// Plain text/code in the editor.
    Text,
    /// Not ours: handed to the system.
    External,
}

/// An open file.
pub struct Doc {
    pub path: PathBuf,
    pub text: String,
    pub kind: DocKind,
    /// Bumped on every edit; the Markdown cache keys off this.
    pub version: u64,
    saved_hash: u64,
    pub saved_mtime: Option<SystemTime>,
    /// Set when the file changed on disk behind our back.
    pub externally_changed: bool,
    /// Reload suggestion, i.e. the file is bigger than we want to edit.
    pub too_large: bool,
    /// The bytes did not look like text. The file is still shown, decoded
    /// lossily, but it opens read-only so nobody can write mangled bytes over
    /// a real file. The app warns when this is set.
    pub looks_binary: bool,
    pub read_only: bool,
}

impl Doc {
    /// Reads a file from disk, returning `None` if it should not be edited here.
    pub fn open(path: &Path) -> Result<Doc, String> {
        let md = fs::metadata(path).map_err(|e| format!("{}: {e}", display(path)))?;
        if md.is_dir() {
            return Err(format!("{} is a folder", display(path)));
        }
        if md.len() > MAX_EDIT_BYTES {
            return Ok(Doc {
                path: path.to_path_buf(),
                text: String::new(),
                kind: DocKind::External,
                version: 0,
                saved_hash: 0,
                saved_mtime: md.modified().ok(),
                externally_changed: false,
                too_large: true,
                looks_binary: false,
                read_only: false,
            });
        }
        let bytes = fs::read(path).map_err(|e| format!("{}: {e}", display(path)))?;
        // Anything opens. A file that does not look like text is shown anyway,
        // decoded lossily, but read-only: writing lossy-decoded bytes back over
        // a real file would destroy it silently.
        let looks_binary = fs_model::is_probably_binary(&bytes);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let hash = hash(&text);
        Ok(Doc {
            path: path.to_path_buf(),
            text,
            kind: doc_kind(path),
            version: 1,
            saved_hash: hash,
            saved_mtime: md.modified().ok(),
            externally_changed: false,
            too_large: false,
            looks_binary,
            read_only: looks_binary || is_read_only(path, &md),
        })
    }

    /// Worker-thread read, for the message bus.
    ///
    /// The whole `Doc` travels, not just its text. Flattening it to a string
    /// dropped `read_only` and `too_large` on the floor, and the app rebuilt
    /// the document as editable — so a large file arrived as an *empty editable
    /// buffer*, and saving it overwrote the file with nothing.
    pub fn read(path: &Path) -> (PathBuf, Option<Doc>, Option<String>) {
        match Doc::open(path) {
            Ok(doc) => (doc.path.clone(), Some(doc), None),
            Err(e) => (path.to_path_buf(), None, Some(e)),
        }
    }

    /// An empty document standing in for a path, used by a folder tab.
    ///
    /// It is never edited or saved: the tab only borrows the path so the strip
    /// can name itself and the watcher knows what to follow.
    pub fn placeholder(path: &Path) -> Doc {
        Doc::from_parts(path.to_path_buf(), String::new(), DocKind::Text, None, true)
    }

    /// Builds a document from text already in memory.
    pub fn from_parts(
        path: PathBuf,
        text: String,
        kind: DocKind,
        saved_mtime: Option<SystemTime>,
        read_only: bool,
    ) -> Doc {
        let hash = hash(&text);
        Doc {
            path,
            text,
            kind,
            version: 1,
            saved_hash: hash,
            saved_mtime,
            externally_changed: false,
            too_large: false,
            looks_binary: false,
            read_only,
        }
    }

    /// Replaces the buffer with the file's current contents.
    pub fn reload(&mut self) -> Result<(), String> {
        let fresh = Doc::open(&self.path)?;
        if fresh.too_large {
            return Err("File is too large to edit".into());
        }
        let version = self.version + 1;
        let read_only = self.read_only;
        *self = fresh;
        self.version = version;
        self.read_only = read_only;
        Ok(())
    }

    /// Writes the buffer back to disk.
    pub fn save(&mut self) -> Result<(), String> {
        if self.too_large {
            return Err("File is too large to save from the editor".into());
        }
        if self.read_only {
            return Err("File is read-only".into());
        }
        fs::write(&self.path, self.text.as_bytes())
            .map_err(|e| format!("{}: {e}", display(&self.path)))?;
        self.saved_hash = hash(&self.text);
        self.saved_mtime = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        self.externally_changed = false;
        Ok(())
    }

    /// `true` when the buffer differs from what is on disk.
    pub fn dirty(&self) -> bool {
        hash(&self.text) != self.saved_hash
    }

    /// Marks the buffer edited so the Markdown cache re-parses.
    pub fn touch(&mut self) {
        self.version += 1;
    }

    /// File name for the tab header.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| display(&self.path))
    }

    /// Relative location, e.g. `../../src/main.rs` from the current folder.
    pub fn location(&self, base: &Path) -> String {
        rel_path(&self.path, base)
    }

    /// Called by the watcher: notes a disk change and whether it matters.
    pub fn check_external_change(&mut self) {
        let Ok(md) = fs::metadata(&self.path) else {
            self.externally_changed = true;
            return;
        };
        let Ok(mtime) = md.modified() else { return };
        if let Some(saved) = self.saved_mtime
            && mtime > saved
            && !self.dirty()
        {
            self.externally_changed = true;
        }
    }
}

/// Which view an opened file gets.
pub fn doc_kind(path: &Path) -> DocKind {
    if is_markdown(path) {
        DocKind::Markdown
    } else {
        DocKind::Text
    }
}

/// Opens a file with the OS default handler.
pub fn open_externally(path: &Path) -> Result<(), String> {
    open::that(path).map_err(|e| format!("Could not open {}: {e}", display(path)))
}

/// Reveals a file in the system file manager.
pub fn reveal_in_file_manager(path: &Path) {
    if let Some(dir) = path.parent() {
        // Best effort: open the containing folder; selecting the file itself is
        // platform specific, so we keep it simple and reliable.
        let _ = open::that(dir);
    }
}

fn is_read_only(_path: &Path, md: &fs::Metadata) -> bool {
    md.permissions().readonly()
}

/// FNV-1a: fast, stable, and only ever used for change detection.
pub fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Shortens a path for display: `../../src/main.rs`.
pub fn rel_path(path: &Path, base: &Path) -> String {
    if let Ok(stripped) = path.strip_prefix(base) {
        let s = stripped.display().to_string();
        if !s.is_empty() {
            return s;
        }
    } // Walk up as far as the two paths agree.
    let a: Vec<_> = path.components().collect();
    let b: Vec<_> = base.components().collect();
    let mut common = 0;
    while common < a.len() && common < b.len() && a[common] == b[common] {
        common += 1;
    }
    let ups = b.len() - common;
    let rest: Vec<String> = a[common..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let mut out = "../".repeat(ups);
    out.push_str(&rest.join("/"));
    if out.is_empty() {
        path.display().to_string()
    } else {
        out
    }
}

fn display(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Editor highlighting profile for a file, or a plain profile.
pub fn syntax_for(path: &Path) -> Syntax {
    match fs_model::ext_of(path).as_str() {
        "rs" => Syntax::rust(),
        "py" | "pyi" => Syntax::python(),
        "lua" => Syntax::lua(),
        "sql" => Syntax::sql(),
        "sh" | "bash" | "zsh" | "fish" => Syntax::shell(),
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "hxx" => Syntax::new("C/C++")
            .with_comment("//")
            .with_comment_multiline(["/*", "*/"]),
        "cs" => Syntax::new("C#")
            .with_comment("//")
            .with_comment_multiline(["/*", "*/"]),
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "json" | "jsonc" => Syntax::new("JavaScript")
            .with_comment("//")
            .with_comment_multiline(["/*", "*/"]),
        "go" => Syntax::new("Go")
            .with_comment("//")
            .with_comment_multiline(["/*", "*/"]),
        "java" | "kt" | "kts" | "scala" => Syntax::new("Java")
            .with_comment("//")
            .with_comment_multiline(["/*", "*/"]),
        "rb" | "php" => Syntax::new("Ruby/PHP")
            .with_comment("//")
            .with_comment_multiline(["/*", "*/"]),
        "toml" | "ini" | "cfg" | "conf" | "properties" => Syntax::new("Config").with_comment("#"),
        "yml" | "yaml" => Syntax::new("YAML").with_comment("#"),
        "html" | "xml" | "svg" | "vue" | "svelte" => Syntax::new("Markup")
            .with_comment("<!--")
            .with_comment_multiline(["<!--", "-->"]),
        "css" | "scss" | "less" => Syntax::simple("/*"),
        _ => Syntax::new("Plain").with_comment("//"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_changes_with_content() {
        assert_ne!(hash("a"), hash("b"));
        assert_eq!(hash("same"), hash("same"));
    }

    #[test]
    fn markdown_and_text_are_distinguished() {
        assert_eq!(doc_kind(Path::new("a/b.md")), DocKind::Markdown);
        assert_eq!(doc_kind(Path::new("a/b.MARKDOWN")), DocKind::Markdown);
        assert_eq!(doc_kind(Path::new("a/b.rs")), DocKind::Text);
    }

    #[test]
    fn save_and_dirty_round_trip() {
        let dir = std::env::temp_dir().join("xplor-doc-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("a.md");
        fs::write(&path, "# hi").unwrap();

        let mut doc = Doc::open(&path).unwrap();
        assert!(!doc.dirty());
        doc.touch();
        assert!(!doc.dirty(), "touching alone must not mark it dirty");
        assert_eq!(doc.file_name(), "a.md");
        doc.save().unwrap();
        assert!(!doc.dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# hi");

        doc.reload().unwrap();
        assert_eq!(doc.text, "# hi");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_binary_file_opens_read_only_instead_of_being_refused() {
        let dir = std::env::temp_dir().join("xplor-doc-bin");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("a.bin");
        fs::write(&path, [0u8, 1, 2, 3]).unwrap();
        let mut doc = Doc::open(&path).expect("any file should open");
        assert!(doc.looks_binary, "the warning flag was not set");
        // The point of opening it at all: you can look at it, but not save
        // lossy decoding over the original.
        assert!(doc.read_only, "a binary file must not be writable");
        let before = fs::read(&path).unwrap();
        assert!(doc.save().is_err(), "saving a binary file was allowed");
        assert_eq!(fs::read(&path).unwrap(), before, "the file was changed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_large_file_cannot_be_saved_over_with_an_empty_buffer() {
        let dir = std::env::temp_dir().join("xplor-doc-large");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("big.log");
        // One byte over the cap, so the size guard is what decides.
        let big = vec![b'x'; crate::fs_model::MAX_EDIT_BYTES as usize + 1];
        fs::write(&path, &big).unwrap();
        let mut doc = Doc::open(&path).expect("a large file should still open");
        assert!(doc.too_large);
        assert!(
            doc.text.is_empty(),
            "a huge file should not be loaded into memory"
        );
        // This is the data-loss case: an empty buffer that must never be
        // written back over the real file.
        assert!(doc.save().is_err(), "saving a too-large file was allowed");
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            big.len() as u64,
            "the file was clobbered"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_worker_read_carries_the_flags_the_app_needs() {
        // `read` used to flatten the document to a string, which silently
        // dropped `read_only` and `too_large`; the app then rebuilt it as an
        // editable buffer.
        let dir = std::env::temp_dir().join("xplor-doc-read");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("a.bin");
        fs::write(&path, [0u8, 1, 2, 3]).unwrap();
        let (_p, doc, err) = Doc::read(&path);
        assert!(err.is_none());
        let doc = doc.expect("the document should travel whole");
        assert!(doc.looks_binary && doc.read_only, "flags lost in transit");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_paths_are_shortened() {
        let file = PathBuf::from("/home/me/docs/notes.md");
        // A direct child of the base shows just its name.
        assert_eq!(rel_path(&file, &PathBuf::from("/home/me/docs")), "notes.md");
        // Anything deeper shows the path from the base.
        assert_eq!(rel_path(&file, &PathBuf::from("/home/me")), "docs/notes.md");
        // Unrelated folders fall back to `..`.
        assert_eq!(
            rel_path(&file, &PathBuf::from("/var/log")),
            "../../home/me/docs/notes.md"
        );
    }
}
