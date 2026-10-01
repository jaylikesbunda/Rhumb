//! Filesystem model: directory entries, sorting, formatting and quick-access
//! locations. All of this is pure logic so it stays cheap and testable.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Files larger than this are never opened in the built-in editor.
pub const MAX_EDIT_BYTES: u64 = 256 * 1024 * 1024;

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
    // An archive, or a place inside one, is read out of the archive.
    if let Some(crate::archive::Inside { archive, inner }) = crate::archive::split(path) {
        let mut entries = crate::archive::list(&archive, &inner)?;
        if !show_hidden {
            entries.retain(|e| !e.hidden);
        }
        return Ok(entries);
    }
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

/// What a background read of the volumes hands back.
type VolumeMap = HashMap<PathBuf, (u64, u64)>;

/// What a background read of the sidebar's entries hands back: label, path, device.
type RootList = Vec<(String, PathBuf, bool)>;

/// Free space per volume, kept in memory and refreshed on a timer.
///
/// Enumerating disks is not cheap: it opens every volume and asks the OS for
/// its capacity, and on a machine with a network share or a sleeping USB disk a
/// single call can take tens of milliseconds or far longer. So it is never done
/// on the thread that draws: a worker does the asking, and the numbers are
/// picked up on the next frame after it answers. They only need to be roughly
/// right, and a few seconds of staleness is invisible.
#[derive(Default)]
pub struct FreeSpace {
    values: HashMap<PathBuf, (u64, u64)>,
    refreshed: Option<std::time::Instant>,
    /// The answer of a refresh in flight, once it has one.
    incoming: std::sync::Arc<std::sync::Mutex<Option<VolumeMap>>>,
    working: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// How long a cached reading is trusted.
const FREE_SPACE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

impl FreeSpace {
    /// Available and total bytes for the volume holding `path`, read right now.
    ///
    /// Waits for the OS, so it is for a worker thread that is about to do something
    /// slow anyway — never for the thread that draws.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn blocking(path: &Path) -> Option<(u64, u64)> {
        let fs = FreeSpace {
            values: read_volumes(),
            ..Default::default()
        };
        fs.lookup(path)
    }

    /// Available and total bytes for the volume holding `path`, from the last
    /// reading. Starts a refresh in the background when that is stale; the call
    /// itself never waits for the OS.
    pub fn get(&mut self, path: &Path, ctx: &egui::Context) -> Option<(u64, u64)> {
        if let Some(fresh) = self.incoming.lock().ok().and_then(|mut g| g.take()) {
            self.values = fresh;
        }
        let stale = self.refreshed.is_none_or(|t| t.elapsed() > FREE_SPACE_TTL);
        if stale
            && !self
                .working
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.refreshed = Some(std::time::Instant::now());
            let incoming = std::sync::Arc::clone(&self.incoming);
            let working = std::sync::Arc::clone(&self.working);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let fresh = read_volumes();
                if let Ok(mut slot) = incoming.lock() {
                    *slot = Some(fresh);
                }
                working.store(false, std::sync::atomic::Ordering::Relaxed);
                ctx.request_repaint();
            });
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
}

/// Every volume's free and total bytes, in one pass over the OS.
fn read_volumes() -> HashMap<PathBuf, (u64, u64)> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|d| {
            (
                PathBuf::from(d.mount_point().to_string_lossy().to_string()),
                (d.available_space(), d.total_space()),
            )
        })
        .collect()
}

/// The sidebar's top-level entries — places and drives — as `(label, path,
/// is_device)`, kept so they are not asked of the OS on every frame.
///
/// The drive list comes from the OS and that can be slow, so it is read once when
/// the sidebar first draws and then again on a timer by a worker, and a frame only
/// ever copies what is already here.
#[derive(Default)]
pub struct Roots {
    list: Vec<(String, PathBuf, bool)>,
    refreshed: Option<std::time::Instant>,
    incoming: std::sync::Arc<std::sync::Mutex<Option<RootList>>>,
    working: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// How long the drive list is trusted before it is read again.
const ROOTS_TTL: std::time::Duration = std::time::Duration::from_secs(5);

impl Roots {
    /// The entries, as of the last reading.
    pub fn get(&mut self, ctx: &egui::Context) -> &[(String, PathBuf, bool)] {
        if self.refreshed.is_none() {
            // The very first frame has nothing to show yet, so it waits once.
            self.list = read_roots();
            self.refreshed = Some(std::time::Instant::now());
        }
        if let Some(fresh) = self.incoming.lock().ok().and_then(|mut g| g.take()) {
            self.list = fresh;
        }
        let stale = self.refreshed.is_none_or(|t| t.elapsed() > ROOTS_TTL);
        if stale
            && !self
                .working
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            self.refreshed = Some(std::time::Instant::now());
            let incoming = std::sync::Arc::clone(&self.incoming);
            let working = std::sync::Arc::clone(&self.working);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let fresh = read_roots();
                if let Ok(mut slot) = incoming.lock() {
                    *slot = Some(fresh);
                }
                working.store(false, std::sync::atomic::Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
        &self.list
    }
}

/// Places, then drives, in the order the sidebar shows them.
fn read_roots() -> Vec<(String, PathBuf, bool)> {
    let mut roots = Vec::new();
    for p in places() {
        let device = p.is_device();
        roots.push((p.label, p.path, device));
    }
    for d in drives() {
        roots.push((d.label, d.path, true));
    }
    roots
}

/// Path components, root first, for the breadcrumb bar.
///
/// `C:\a\b` becomes `[(C:, C:\), (a, C:\a), (b, C:\a\b)]`.
pub fn breadcrumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut parts: Vec<(String, PathBuf)> = Vec::new();
    let mut acc = PathBuf::new();
    let comps: Vec<_> = path.components().collect();
    // Whether a segment has already named the root, so the one after it can be
    // recognised as the same place rather than shown as a second name for it.
    let mut named_root = false;
    for (i, c) in comps.iter().enumerate() {
        acc.push(c.as_os_str());
        let is_last = i + 1 == comps.len();
        let label = match c {
            std::path::Component::Prefix(p) => {
                named_root = true;
                p.as_os_str().to_string_lossy().into_owned()
            }
            // On Windows the root directory follows the drive prefix, and its label
            // comes from the path so far — which is `C:\`, so it renders as `C`
            // again. The drive has already said where this is, so this segment
            // contributes nothing to show. It is still pushed with an empty label so
            // that the address bar's separators land in the right places, and
            // dropped below, because an empty label is a separator and not a
            // segment.
            std::path::Component::RootDir if named_root => {
                parts.push((String::new(), acc.clone()));
                if is_last {
                    // `C:\` is the whole path, so the last segment is the blank one
                    // and there is nothing after it to separate. Hand it the drive's
                    // label instead, which is what a reader expects to see for a
                    // root they have arrived at.
                    if let Some(first) = parts.first_mut()
                        && first.0.is_empty()
                    {
                        first.0 = root_label(&acc);
                        return parts.into_iter().filter(|p| !p.0.is_empty()).collect();
                    }
                }
                continue;
            }
            std::path::Component::RootDir => {
                named_root = true;
                root_label(&acc)
            }
            std::path::Component::CurDir => ".".to_owned(),
            std::path::Component::ParentDir => "..".to_owned(),
            _ => c.as_os_str().to_string_lossy().into_owned(),
        };
        if !is_last || !label.is_empty() {
            parts.push((label, acc.clone()));
        }
    }
    let mut shown: Vec<(String, PathBuf)> = parts.into_iter().filter(|p| !p.0.is_empty()).collect();
    // A drive on its own, `C:`, names the folder a process is in *on that drive*, not
    // its root, so a segment that stops there would take a click to somewhere else.
    if let Some(first) = shown.first_mut()
        && cfg!(windows)
        && !first.1.has_root()
        && first.1.to_string_lossy().ends_with(':')
    {
        first.1 = PathBuf::from(format!("{}\\", first.1.display()));
    }
    if shown.is_empty() {
        shown.push((root_label(path), PathBuf::from("/")));
    }
    shown
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

/// Whether these bytes are probably not text.
///
/// A NUL byte in the first few KB is the usual giveaway. The exception is
/// a UTF-16 byte-order mark: that is text, and every second byte in it is
/// NUL, so without this a UTF-16 file reads as binary and loses the editor.
pub fn is_probably_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.starts_with(&[0xFF, 0xFE]) || probe.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
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

/// `cargo test --release probe_sidebar_root_costs -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn probe_sidebar_root_costs() {
    use std::time::Instant;
    for (name, f) in [
        ("places()", (|| drop(places())) as fn()),
        ("drives()", || drop(drives())),
    ] {
        let mut times = Vec::new();
        for _ in 0..30 {
            let t = Instant::now();
            f();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        eprintln!(
            "{name:10} median {:.2} ms  max {:.2} ms",
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_text_is_not_mistaken_for_binary() {
        // "hi" as UTF-16 LE with a BOM: every second byte is NUL, so a plain
        // NUL sniff would refuse the editor to a text file.
        let utf16 = [0xFF, 0xFE, b'h', 0, b'i', 0, 0, 0];
        assert!(!is_probably_binary(&utf16));
        let utf16_be = [0xFE, 0xFF, 0, b'h', 0, b'i'];
        assert!(!is_probably_binary(&utf16_be));
    }

    #[test]
    fn nul_bytes_mean_binary() {
        assert!(is_probably_binary(b"MZ\x90\0\0\0"));
        assert!(!is_probably_binary(b"plain ascii"));
        assert!(!is_probably_binary("héllo, wörld".as_bytes()));
        // A NUL past the probe window is not enough to call it binary.
        let mut late = vec![b'a'; 9000];
        late[8500] = 0;
        assert!(!is_probably_binary(&late));
    }

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
        let dir = std::env::temp_dir().join("rhumb-test-unique");
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
        // First segment is the root, last is the folder itself. The root of a drive is
        // `C:\`: `C:` alone means the current folder on that drive, so a click on it
        // went somewhere other than the top of the drive.
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        assert_eq!(bc.first().unwrap().1, root);
        assert!(bc.first().unwrap().1.has_root(), "{bc:?}");
        assert_eq!(bc.last().unwrap().1, p);
        assert_eq!(bc.last().unwrap().0, "docs");
        assert!(bc.len() >= 2, "{bc:?}");
    }

    #[test]
    fn a_drive_segment_of_the_address_bar_leads_to_the_root_of_the_drive() {
        if !cfg!(windows) {
            return;
        }
        for path in [r"D:\", r"D:", r"D:\c"] {
            let bc = breadcrumbs(Path::new(path));
            assert_eq!(bc[0].0, "D:", "{bc:?}");
            assert_eq!(bc[0].1, PathBuf::from(r"D:\"), "{path}");
        }
    }

    #[test]
    fn the_drive_letter_is_not_shown_twice() {
        // On Windows a path has *two* components at the front: the prefix (`C:`) and
        // the root directory (`\`). The prefix already reads as "C:", and labelling
        // the root as well — which is derived from the accumulated path and so comes
        // out as "C" — puts the drive letter in the address bar twice, as
        // `C: > C > Users > dev`. Only the prefix should be shown, and only when
        // there is one.
        let p = if cfg!(windows) {
            PathBuf::from(r"C:\Users\dev\docs")
        } else {
            PathBuf::from("/home/dev/docs")
        };
        let labels: Vec<String> = breadcrumbs(&p).into_iter().map(|(l, _)| l).collect();
        let dupes = labels.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(dupes, 0, "the same label twice in a row: {labels:?}");
        if cfg!(windows) {
            assert_eq!(labels.first().map(String::as_str), Some("C:"));
            assert_eq!(
                labels.iter().filter(|l| l.starts_with('C')).count(),
                1,
                "the drive letter appears once: {labels:?}"
            );
        }
        // And the first segment must still navigate to the root, so a reader can get
        // back there by clicking it.
        let bc = breadcrumbs(&p);
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        assert_eq!(
            bc.first().unwrap().1,
            root,
            "the first segment goes to the root of the drive"
        );
    }

    #[test]
    fn a_bare_drive_root_reads_as_the_drive() {
        let p = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        let labels: Vec<String> = breadcrumbs(&p).into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels.len(), 1, "one segment for a bare root: {labels:?}");
        let want = if cfg!(windows) { "C:" } else { "/" };
        assert_eq!(labels[0], want);
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
