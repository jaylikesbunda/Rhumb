//! Filesystem model: directory entries, sorting, formatting and quick-access
//! locations. All of this is pure logic so it stays cheap and testable.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Files larger than this are never opened in the built-in editor.
pub const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;

/// Sort column for the file list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Ext,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Modified => "Modified",
            SortKey::Ext => "Type",
        }
    }
}

/// One row in the file list.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Kept for completeness; hidden entries are filtered at read time.
    #[allow(dead_code)]
    pub hidden: bool,
}

impl Entry {
    pub fn ext(&self) -> String {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    }
}

/// Reads a directory into entries, skipping hidden files unless asked.
///
/// Runs on a worker thread; touches the disk exactly once.
pub fn read_dir(path: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::with_capacity(64);
    for item in fs::read_dir(path)? {
        let Ok(item) = item else { continue };
        let name_os = item.file_name();
        let Some(name) = name_os.to_str() else {
            continue; // non-UTF-8 name: skip rather than mangle
        };
        // One stat per entry. `is_hidden` used to fetch metadata of its own,
        // which doubled the work of listing a folder holding hundreds of
        // thousands of files. Metadata errors are common (broken links,
        // races); fall back to defaults.
        let md = item.metadata().ok();
        let is_hidden = is_hidden(name, md.as_ref());
        if is_hidden && !show_hidden {
            continue;
        }
        let is_symlink = md.as_ref().is_some_and(fs::Metadata::is_symlink);
        let is_dir = md.as_ref().map_or_else(
            || item.file_type().is_ok_and(|t| t.is_dir()),
            fs::Metadata::is_dir,
        );
        let size = md.as_ref().map_or(0, fs::Metadata::len);
        let modified = md.as_ref().and_then(|m| m.modified().ok());
        out.push(Entry {
            name: name.to_owned(),
            path: item.path(),
            is_dir,
            is_symlink,
            size,
            modified,
            hidden: is_hidden,
        });
    }
    Ok(out)
}

/// Hidden = dot-prefixed, or (on Windows) the hidden file attribute.
///
/// Takes the metadata the listing already fetched: asking for it again per
/// entry is what made reading a very large folder slow.
pub fn is_hidden(name: &str, md: Option<&fs::Metadata>) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        md.is_some_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = md;
        false
    }
}

/// Sorts in place: directories first, then by the chosen key.
pub fn sort(entries: &mut Vec<Entry>, key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| dir_first(a, b).then_with(|| cmp_key(a, b, key)));
    if !ascending && key != SortKey::Name {
        // Only the secondary key is reversed; folders stay on top.
        let mut files: Vec<Entry> = entries.iter().filter(|e| !e.is_dir).cloned().collect();
        files.reverse();
        entries.retain(|e| e.is_dir);
        entries.extend(files);
    }
}

fn dir_first(a: &Entry, b: &Entry) -> Ordering {
    b.is_dir.cmp(&a.is_dir)
}

fn cmp_key(a: &Entry, b: &Entry, key: SortKey) -> Ordering {
    match key {
        SortKey::Name => natural_cmp(&a.name, &b.name),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.modified.cmp(&b.modified),
        SortKey::Ext => {
            let (ae, be) = (a.ext(), b.ext());
            natural_cmp(&ae, &be).then_with(|| natural_cmp(&a.name, &b.name))
        }
    }
}

/// Case-insensitive natural order, so `file2` sorts before `file10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let na = take_number(&mut ai);
                    let nb = take_number(&mut bi);
                    // Compare numerically, ignoring leading zeros.
                    match na.len().cmp(&nb.len()).then_with(|| na.cmp(&nb)) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    let lx = x.to_ascii_lowercase();
                    let ly = y.to_ascii_lowercase();
                    if lx != ly {
                        return lx.cmp(&ly);
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    while let Some(c) = it.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        out.push(c);
        it.next();
    }
    out
}

/// Human-readable byte size, e.g. `1.4 MB`.
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Formats a timestamp in local time.
///
/// Files touched in the current year show `MMM DD  HH:MM`, older ones show
/// `MMM DD  YYYY`. The year is always read from the system clock, never
/// hardcoded, so the app stays correct as time passes.
pub fn fmt_time(t: SystemTime) -> String {
    let Ok(secs) = t.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let secs = secs.as_secs() as i64;
    let Some(t) = jiff::Timestamp::from_second(secs).ok() else {
        return String::new();
    };
    let zoned = t.to_zoned(jiff::tz::TimeZone::system());
    let dt = zoned.datetime();
    let this_year = current_year();
    if dt.year() == this_year {
        format!(
            "{} {:02}  {:02}:{:02}",
            MONTHS[(dt.month() - 1) as usize],
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    } else {
        format!(
            "{} {:02}  {}",
            MONTHS[(dt.month() - 1) as usize],
            dt.day(),
            dt.year()
        )
    }
}

/// Current calendar year in local time, queried live.
pub fn current_year() -> i16 {
    jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::system())
        .datetime()
        .year()
}

/// A sidebar shortcut: a label and the path it points at.
#[derive(Clone, Debug)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    /// Used by the sidebar to pick an icon.
    pub kind: PlaceKind,
}

impl Place {
    /// Drives are drawn with a different glyph from folders.
    pub fn is_device(&self) -> bool {
        self.kind == PlaceKind::Drive
    }
}

/// What kind of place a sidebar row points at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaceKind {
    Home,
    Directory,
    Drive,
}

/// User folders and mounted drives, in display order.
pub fn places() -> Vec<Place> {
    let mut out = Vec::new();
    let mut push = |label: &str, path: Option<PathBuf>, kind: PlaceKind| {
        if let Some(path) = path {
            out.push(Place {
                label: label.to_owned(),
                path,
                kind,
            });
        }
    };
    push("Home", dirs::home_dir(), PlaceKind::Home);
    push("Desktop", dirs::desktop_dir(), PlaceKind::Directory);
    push("Documents", dirs::document_dir(), PlaceKind::Directory);
    push("Downloads", dirs::download_dir(), PlaceKind::Directory);
    push("Pictures", dirs::picture_dir(), PlaceKind::Directory);
    push("Music", dirs::audio_dir(), PlaceKind::Directory);
    push("Videos", dirs::video_dir(), PlaceKind::Directory);
    out
}

/// Mounted volumes. Refreshed on demand because it touches the OS.
pub fn drives() -> Vec<Place> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let mut out = Vec::new();
    for d in disks.list() {
        let mount = d.mount_point();
        // Skip pseudo / duplicate mounts so the list stays short and useful.
        let name = d.name().to_string_lossy().to_string();
        let label = if name.is_empty() {
            mount.display().to_string()
        } else {
            name
        };
        if out.iter().any(|p: &Place| p.path == mount) {
            continue;
        }
        out.push(Place {
            label,
            path: mount.to_path_buf(),
            kind: PlaceKind::Drive,
        });
    }
    out
}

/// Free space per volume, kept in memory and refreshed on a timer.
///
/// Enumerating disks is not cheap: it opens every volume and asks the OS for
/// its capacity. Doing that once per device row per frame burned most of a core
/// while the app sat idle, so it is cached here instead. The numbers only need
/// to be roughly right, and a few seconds of staleness is invisible.
#[derive(Default)]
pub struct FreeSpace {
    values: HashMap<PathBuf, (u64, u64)>,
    refreshed: Option<std::time::Instant>,
}

/// How long a cached reading is trusted.
const FREE_SPACE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

impl FreeSpace {
    /// Available and total bytes for the volume holding `path`.
    pub fn get(&mut self, path: &Path) -> Option<(u64, u64)> {
        let stale = self.refreshed.is_none_or(|t| t.elapsed() > FREE_SPACE_TTL);
        if stale {
            self.refresh();
        }
        self.lookup(path)
    }

    /// Finds the volume a path sits on, without touching the OS.
    ///
    /// The longest matching mount point wins, so `/media/usb/photos` resolves
    /// to `/media/usb` rather than to the root.
    fn lookup(&self, path: &Path) -> Option<(u64, u64)> {
        if let Some(v) = self.values.get(path) {
            return Some(*v);
        }
        self.values
            .iter()
            .filter(|(mount, _)| path.starts_with(mount))
            .max_by_key(|(mount, _)| mount.components().count())
            .map(|(_, v)| *v)
    }

    /// Re-reads every volume in one pass.
    pub fn refresh(&mut self) {
        use sysinfo::Disks;
        let disks = Disks::new_with_refreshed_list();
        self.values.clear();
        for d in disks.list() {
            let mount = PathBuf::from(d.mount_point().to_string_lossy().to_string());
            self.values
                .insert(mount, (d.available_space(), d.total_space()));
        }
        self.refreshed = Some(std::time::Instant::now());
    }
}

/// Path components, root first, for the breadcrumb bar.
///
/// `C:\a\b` becomes `[(C:, C:\), (a, C:\a), (b, C:\a\b)]`.
pub fn breadcrumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    let mut acc = PathBuf::new();
    let comps: Vec<_> = path.components().collect();
    for (i, c) in comps.iter().enumerate() {
        acc.push(c.as_os_str());
        let label = match c {
            std::path::Component::RootDir => root_label(&acc),
            std::path::Component::Prefix(p) => p.as_os_str().to_string_lossy().into_owned(),
            std::path::Component::CurDir => ".".to_owned(),
            std::path::Component::ParentDir => "..".to_owned(),
            _ => c.as_os_str().to_string_lossy().into_owned(),
        };
        let is_last = i + 1 == comps.len();
        if !is_last || !label.is_empty() {
            parts.push((label, acc.clone()));
        }
    }
    if parts.is_empty() {
        parts.push((root_label(path), PathBuf::from("/")));
    }
    parts
}

fn root_label(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC") {
        let _ = rest;
        return "\\\\".to_owned();
    }
    if s.starts_with("\\\\") || s.starts_with("//") {
        return "//".to_owned();
    }
    // `C:\` -> `C:`
    let t = s.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        if cfg!(windows) {
            "C:".to_owned()
        } else {
            "/".to_owned()
        }
    } else {
        t.to_owned()
    }
}

/// `file.txt` -> `txt`, empty when there is none.
pub fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Resolves a possibly-relative user-typed path against a base directory.
pub fn resolve_input(input: &str, base: &Path) -> Option<PathBuf> {
    let trimmed = input.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let mut p = PathBuf::from(trimmed);
    if p.is_relative() {
        p = base.join(p);
    }
    Some(normalize(&p))
}

/// Lexically cleans a path: resolves `.` and `..` without touching the disk.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Picks a non-colliding destination: `a.txt` -> `a (2).txt`.
pub fn unique_dest(dest: &Path) -> PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }
    let parent = dest.parent().unwrap_or(Path::new("."));
    let stem = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    let ext = dest.extension().and_then(|s| s.to_str());
    for n in 2..10_000 {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dest.to_path_buf()
}

/// Validates a new-name typed by the user. Returns a message on failure.
pub fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if trimmed.len() > 255 {
        return Err("Name is too long".into());
    }
    if trimmed == "." || trimmed == ".." {
        return Err(format!("{trimmed} is not a valid name"));
    }
    if trimmed.contains(['/', '\\']) {
        return Err("Name cannot contain \\ or /".into());
    }
    if cfg!(windows) {
        let bad = ['<', '>', ':', '"', '|', '?', '*'];
        if trimmed.contains(bad) {
            return Err("Name contains characters Windows does not allow".into());
        }
        let reserved = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
        if reserved.contains(&stem.as_str()) {
            return Err(format!("{stem} is a reserved name on Windows"));
        }
    }
    Ok(())
}

/// True when the file looks like text we can safely show in the editor.
pub fn is_probably_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8192)];
    probe.contains(&0)
}

/// Extensions we open in the built-in editor.
const TEXT_EXTS: &[&str] = &[
    "txt",
    "text",
    "log",
    "ini",
    "cfg",
    "conf",
    "toml",
    "yaml",
    "yml",
    "json",
    "jsonc",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "less",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "tsx",
    "rs",
    "py",
    "go",
    "java",
    "kt",
    "kts",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "rb",
    "php",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "lua",
    "pl",
    "r",
    "jl",
    "swift",
    "dart",
    "vue",
    "svelte",
    "zig",
    "nim",
    "hs",
    "ex",
    "exs",
    "erl",
    "clj",
    "scala",
    "vue",
    "gitignore",
    "env",
    "lock",
    "diff",
    "patch",
    "csv",
    "tsv",
    "srt",
    "graphql",
    "proto",
    "dockerfile",
    "makefile",
    "editorconfig",
];

/// Extensions rendered as Markdown in the preview pane.
const MD_EXTS: &[&str] = &["md", "markdown", "mdown", "mkd", "mdx", "rmd"];

/// Is this a Markdown document?
pub fn is_markdown(path: &Path) -> bool {
    MD_EXTS.contains(&ext_of(path).as_str())
}

/// Should the built-in editor open this file?
pub fn is_editable_text(path: &Path) -> bool {
    if is_markdown(path) {
        return true;
    }
    let ext = ext_of(path);
    if TEXT_EXTS.contains(&ext.as_str()) {
        return true;
    }
    // Extension-less files that are common config or docs (Dockerfile, LICENSE…).
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(
            "Dockerfile"
                | "Makefile"
                | "LICENSE"
                | "README"
                | "CHANGELOG"
                | "Justfile"
                | ".gitignore"
                | ".env"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_orders_numbers_numerically() {
        let mut v = vec!["file10", "file2", "file1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["file1", "file2", "file10"]);
    }

    #[test]
    fn natural_sort_ignores_case() {
        assert_eq!(natural_cmp("Alpha", "alpha"), Ordering::Equal);
        assert_eq!(natural_cmp("Alpha2", "alpha10"), Ordering::Less);
    }

    #[test]
    fn size_formatting_is_readable() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(999), "999 B");
        assert_eq!(fmt_size(1024), "1.00 KB");
        assert_eq!(fmt_size(1536), "1.50 KB");
        assert_eq!(fmt_size(1024 * 1024 * 3), "3.00 MB");
    }

    #[test]
    fn unique_dest_avoids_collisions() {
        let dir = std::env::temp_dir().join("xplor-test-unique");
        let _ = fs::create_dir_all(&dir);
        let base = dir.join("a.txt");
        let _ = fs::write(&base, "x");
        assert_eq!(unique_dest(&base), dir.join("a (2).txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_validation_rejects_bad_input() {
        assert!(validate_name("ok").is_ok());
        assert!(validate_name("  ").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("x*").is_err());
    }

    #[test]
    fn breadcrumbs_walk_from_root() {
        let p = if cfg!(windows) {
            PathBuf::from(r"C:\Users\dev\docs")
        } else {
            PathBuf::from("/home/dev/docs")
        };
        let bc = breadcrumbs(&p);
        // First segment is the root, last is the folder itself.
        assert_eq!(
            bc.first().unwrap().1,
            PathBuf::from(p.components().next().unwrap().as_os_str())
        );
        assert_eq!(bc.last().unwrap().1, p);
        assert_eq!(bc.last().unwrap().0, "docs");
        assert!(bc.len() >= 2, "{bc:?}");
    }

    #[test]
    fn normalize_resolves_dot_segments() {
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn binary_detection() {
        assert!(is_probably_binary(b"abc\0def"));
        assert!(!is_probably_binary(b"hello world\n"));
    }

    #[test]
    fn current_year_is_sane() {
        let y = current_year();
        assert!((2020..=2100).contains(&y), "unexpected year {y}");
    }
}
