//! Filesystem model: directory entries, sorting, formatting and quick-access
//! locations. All of this is pure logic so it stays cheap and testable.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::io;
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

/// How the file list is broken into headed groups.
///
/// The heading is drawn before the first row of each group and is not a row
/// itself: it cannot be selected, clicked or reached with the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GroupBy {
    #[default]
    None,
    /// A-Z buckets, with anything not starting with a letter under `#`.
    Name,
    /// Folders, then the broad families of extension.
    Type,
    /// Today / Yesterday / Earlier this week / Earlier this month / A long time ago.
    Modified,
    /// Folders, then size bands.
    Size,
    /// This PC only: the user folders, then the volumes. Not a menu choice; the
    /// place view is what This PC always uses.
    Place,
}

impl GroupBy {
    /// The choices the menu offers, in the order it shows them.
    pub const ALL: [GroupBy; 5] = [
        GroupBy::None,
        GroupBy::Name,
        GroupBy::Type,
        GroupBy::Modified,
        GroupBy::Size,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::None => "None",
            GroupBy::Name => "Name",
            GroupBy::Type => "Type",
            GroupBy::Modified => "Modified",
            GroupBy::Size => "Size",
            GroupBy::Place => "Place",
        }
    }

    /// The name written to prefs.
    pub fn key(self) -> &'static str {
        match self {
            GroupBy::None => "none",
            GroupBy::Name => "name",
            GroupBy::Type => "type",
            GroupBy::Modified => "modified",
            GroupBy::Size => "size",
            GroupBy::Place => "place",
        }
    }

    pub fn from_key(key: &str) -> GroupBy {
        match key {
            "name" => GroupBy::Name,
            "type" => GroupBy::Type,
            "modified" => GroupBy::Modified,
            "size" => GroupBy::Size,
            _ => GroupBy::None,
        }
    }
}

/// The broad family a file's extension belongs to.
///
/// Folders are not files: the caller knows `is_dir` and handles them itself,
/// so this never has to guess from a name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Document,
    Image,
    Audio,
    Video,
    Archive,
    Other,
}

/// Classifies an extension, as [`ext_of`] returns it: lower-case and without
/// the dot. Anything not recognised is [`FileKind::Other`], which is most of
/// what a folder holds.
pub fn kind_of(ext: &str) -> FileKind {
    const DOCUMENTS: &[&str] = &[
        "txt", "text", "log", "md", "markdown", "mdown", "mkd", "mdx", "rmd", "rtf", "doc", "docx",
        "odt", "pdf", "xls", "xlsx", "ods", "ppt", "pptx", "odp", "csv", "tsv", "epub",
    ];
    const IMAGES: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "svg", "ico", "heic", "avif",
        "raw",
    ];
    const AUDIO: &[&str] = &[
        "mp3", "wav", "flac", "aac", "ogg", "oga", "opus", "m4a", "wma", "aiff", "mid", "midi",
    ];
    const VIDEO: &[&str] = &[
        "mp4", "mkv", "mov", "avi", "wmv", "webm", "flv", "m4v", "mpg", "mpeg", "3gp",
    ];
    const ARCHIVES: &[&str] = &[
        "zip", "zipx", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "xz", "txz", "zst", "lz",
        "lzma", "cab", "iso", "jar", "war", "apk",
    ];
    let ext = ext.to_ascii_lowercase();
    if DOCUMENTS.contains(&ext.as_str()) {
        FileKind::Document
    } else if IMAGES.contains(&ext.as_str()) {
        FileKind::Image
    } else if AUDIO.contains(&ext.as_str()) {
        FileKind::Audio
    } else if VIDEO.contains(&ext.as_str()) {
        FileKind::Video
    } else if ARCHIVES.contains(&ext.as_str()) {
        FileKind::Archive
    } else {
        FileKind::Other
    }
}

/// One megabyte, the unit the size bands are named in.
pub const MB: u64 = 1024 * 1024;

/// What kinds of entry the kind filter keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KindFilter {
    #[default]
    All,
    Folders,
    /// Every non-folder. The extension choices below are subsets of it.
    Files,
    Documents,
    Images,
    Audio,
    Video,
    Archives,
}

impl KindFilter {
    pub const ALL: [KindFilter; 8] = [
        KindFilter::All,
        KindFilter::Folders,
        KindFilter::Files,
        KindFilter::Documents,
        KindFilter::Images,
        KindFilter::Audio,
        KindFilter::Video,
        KindFilter::Archives,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "All",
            KindFilter::Folders => "Folders",
            KindFilter::Files => "Files",
            KindFilter::Documents => "Documents",
            KindFilter::Images => "Images",
            KindFilter::Audio => "Audio",
            KindFilter::Video => "Video",
            KindFilter::Archives => "Archives",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            KindFilter::All => "all",
            KindFilter::Folders => "folders",
            KindFilter::Files => "files",
            KindFilter::Documents => "documents",
            KindFilter::Images => "images",
            KindFilter::Audio => "audio",
            KindFilter::Video => "video",
            KindFilter::Archives => "archives",
        }
    }

    pub fn from_key(key: &str) -> KindFilter {
        match key {
            "folders" => KindFilter::Folders,
            "files" => KindFilter::Files,
            "documents" => KindFilter::Documents,
            "images" => KindFilter::Images,
            "audio" => KindFilter::Audio,
            "video" => KindFilter::Video,
            "archives" => KindFilter::Archives,
            _ => KindFilter::All,
        }
    }

    /// Whether an entry passes. Folders are only kept by `Folders` and `All`;
    /// the extension choices apply to files.
    pub fn accepts(self, entry: &Entry) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Folders => entry.is_dir,
            KindFilter::Files => !entry.is_dir,
            KindFilter::Documents => is_kind(entry, FileKind::Document),
            KindFilter::Images => is_kind(entry, FileKind::Image),
            KindFilter::Audio => is_kind(entry, FileKind::Audio),
            KindFilter::Video => is_kind(entry, FileKind::Video),
            KindFilter::Archives => is_kind(entry, FileKind::Archive),
        }
    }
}

fn is_kind(entry: &Entry, want: FileKind) -> bool {
    !entry.is_dir && kind_of(&entry.ext()) == want
}

/// How recent an entry must be to pass the modified filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DateFilter {
    #[default]
    Any,
    Today,
    Last7,
    Last30,
    ThisYear,
}

impl DateFilter {
    pub const ALL: [DateFilter; 5] = [
        DateFilter::Any,
        DateFilter::Today,
        DateFilter::Last7,
        DateFilter::Last30,
        DateFilter::ThisYear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateFilter::Any => "Any time",
            DateFilter::Today => "Today",
            DateFilter::Last7 => "Last 7 days",
            DateFilter::Last30 => "Last 30 days",
            DateFilter::ThisYear => "This year",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            DateFilter::Any => "any",
            DateFilter::Today => "today",
            DateFilter::Last7 => "last7",
            DateFilter::Last30 => "last30",
            DateFilter::ThisYear => "thisyear",
        }
    }

    pub fn from_key(key: &str) -> DateFilter {
        match key {
            "today" => DateFilter::Today,
            "last7" => DateFilter::Last7,
            "last30" => DateFilter::Last30,
            "thisyear" => DateFilter::ThisYear,
            _ => DateFilter::Any,
        }
    }

    /// Whether an entry passes. An entry with no usable date fails every choice
    /// but `Any`, since nothing about it can be shown to be recent.
    pub fn accepts(self, entry: &Entry, now: SystemTime) -> bool {
        match self {
            DateFilter::Any => true,
            DateFilter::Today => days_ago(entry.modified, now).is_some_and(|d| d <= 0),
            DateFilter::Last7 => days_ago(entry.modified, now).is_some_and(|d| d <= 6),
            DateFilter::Last30 => days_ago(entry.modified, now).is_some_and(|d| d <= 29),
            DateFilter::ThisYear => same_year(entry.modified, now),
        }
    }
}

/// How big an entry must be to pass the size filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SizeFilter {
    #[default]
    Any,
    Small,
    Medium,
    Large,
}

impl SizeFilter {
    pub const ALL: [SizeFilter; 4] = [
        SizeFilter::Any,
        SizeFilter::Small,
        SizeFilter::Medium,
        SizeFilter::Large,
    ];

    /// The choices are bands, not ceilings: Small is under 1 MB, Medium is
    /// 1 MB up to 100 MB, and Large is 100 MB and over.
    pub fn label(self) -> &'static str {
        match self {
            SizeFilter::Any => "Any",
            SizeFilter::Small => "Small (< 1 MB)",
            SizeFilter::Medium => "Medium (< 100 MB)",
            SizeFilter::Large => "Large (\u{2265} 100 MB)",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SizeFilter::Any => "any",
            SizeFilter::Small => "small",
            SizeFilter::Medium => "medium",
            SizeFilter::Large => "large",
        }
    }

    pub fn from_key(key: &str) -> SizeFilter {
        match key {
            "small" => SizeFilter::Small,
            "medium" => SizeFilter::Medium,
            "large" => SizeFilter::Large,
            _ => SizeFilter::Any,
        }
    }

    /// Whether an entry passes. Folders are exempt: a folder's own byte count
    /// is not the size of what is in it, so the filter never hides a folder.
    pub fn accepts(self, entry: &Entry) -> bool {
        if entry.is_dir || self == SizeFilter::Any {
            return true;
        }
        match self {
            SizeFilter::Any => true,
            SizeFilter::Small => entry.size < MB,
            SizeFilter::Medium => (MB..100 * MB).contains(&entry.size),
            SizeFilter::Large => entry.size >= 100 * MB,
        }
    }
}

/// The heading an entry falls under for the chosen grouping.
///
/// Only the date grouping reads the clock; the others do not ask for it.
pub fn group_label(entry: &Entry, group_by: GroupBy) -> String {
    match group_by {
        GroupBy::None => String::new(),
        GroupBy::Name => name_group(&entry.name),
        GroupBy::Type => type_group(entry).to_owned(),
        GroupBy::Modified => modified_group(entry.modified, SystemTime::now()).to_owned(),
        GroupBy::Size => size_group(entry).to_owned(),
        GroupBy::Place => place_group(entry),
    }
}

/// The This PC grouping: the user folders, then the volumes.
///
/// A volume is a root with nothing above it; every user folder has a parent, so
/// the two are told apart by the path and nothing new has to be carried on the
/// entry.
fn place_group(entry: &Entry) -> String {
    if entry.path.parent().is_none() {
        String::from("Devices and drives")
    } else {
        String::from("Folders")
    }
}

/// Sort position of a group heading, so the list shows groups in a fixed,
/// familiar order rather than in whatever order the entries happened to be in.
pub fn group_rank(group_by: GroupBy, label: &str) -> usize {
    match group_by {
        GroupBy::None => 0,
        GroupBy::Name => {
            let first = label.chars().next().unwrap_or('#').to_ascii_uppercase();
            if first.is_ascii_alphabetic() {
                // The `#` bucket leads, the way the name sort puts digits and
                // symbols before letters.
                (first as usize) - ('A' as usize) + 1
            } else {
                0
            }
        }
        GroupBy::Type => rank_in(
            label,
            &[
                "Folders",
                "Documents",
                "Images",
                "Audio",
                "Video",
                "Archives",
                "Other",
            ],
        ),
        GroupBy::Modified => rank_in(
            label,
            &[
                "Today",
                "Yesterday",
                "Earlier this week",
                "Earlier this month",
                "A long time ago",
            ],
        ),
        GroupBy::Size => rank_in(label, &["Folders", "Small", "Medium", "Large"]),
        GroupBy::Place => usize::from(label != "Folders"),
    }
}

fn rank_in(label: &str, order: &[&str]) -> usize {
    order
        .iter()
        .position(|o| *o == label)
        .unwrap_or(order.len())
}

/// A-Z buckets, with anything not starting with a letter under `#`.
fn name_group(name: &str) -> String {
    let first = name.chars().next().map(|c| c.to_ascii_uppercase());
    match first {
        Some(c) if c.is_ascii_alphabetic() => c.to_string(),
        _ => "#".to_owned(),
    }
}

/// Folders, then the broad families of extension.
fn type_group(entry: &Entry) -> &'static str {
    if entry.is_dir {
        return "Folders";
    }
    match kind_of(&entry.ext()) {
        FileKind::Document => "Documents",
        FileKind::Image => "Images",
        FileKind::Audio => "Audio",
        FileKind::Video => "Video",
        FileKind::Archive => "Archives",
        FileKind::Other => "Other",
    }
}

/// Size bands, matching the size filter: folders first, then by the file's own
/// bytes.
fn size_group(entry: &Entry) -> &'static str {
    if entry.is_dir {
        return "Folders";
    }
    if entry.size < MB {
        "Small"
    } else if entry.size < 100 * MB {
        "Medium"
    } else {
        "Large"
    }
}

/// The date bucket an entry falls in, relative to `now`.
///
/// "This week" is the last seven days and "this month" the last thirty, which
/// is what the filter choices mean as well, so the two agree.
pub fn modified_group(t: Option<SystemTime>, now: SystemTime) -> &'static str {
    bucket_days(days_ago(t, now).unwrap_or(i64::MAX))
}

/// The name of the bucket `days` whole days back falls in. `i64::MAX` means the
/// date is unknown, which is treated as the oldest.
fn bucket_days(days: i64) -> &'static str {
    match days {
        i64::MIN..=0 => "Today",
        1 => "Yesterday",
        2..=6 => "Earlier this week",
        7..=29 => "Earlier this month",
        _ => "A long time ago",
    }
}

/// Whole local days between an entry's modified date and `now`, negative for
/// the future. `None` when either date cannot be read.
pub fn days_ago(t: Option<SystemTime>, now: SystemTime) -> Option<i64> {
    let date = civil_date(t?)?;
    let today = civil_date(now)?;
    Some(civil_day_number(today) - civil_day_number(date))
}

fn same_year(t: Option<SystemTime>, now: SystemTime) -> bool {
    let Some(t) = t else { return false };
    match (civil_date(t), civil_date(now)) {
        (Some(a), Some(b)) => a.0 == b.0,
        _ => false,
    }
}

/// The local calendar date of a timestamp as `(year, month, day)`.
fn civil_date(t: SystemTime) -> Option<(i16, i8, i8)> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let ts = jiff::Timestamp::from_second(secs).ok()?;
    let dt = ts.to_zoned(jiff::tz::TimeZone::system()).datetime();
    Some((dt.year(), dt.month(), dt.day()))
}

/// Days since 1970-01-01 for a civil date. Howard Hinnant's `days_from_civil`,
/// exact for the whole range and needing no calendar library.
fn civil_day_number((y, m, d): (i16, i8, i8)) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
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

/// A place the file list can show.
///
/// Most places are a folder on a disk, but a few name no folder at all: the
/// Recycle Bin, "This PC", and a path inside an archive. This is the one place
/// that turns such a path back into something listable, so everything above it
/// can keep treating a place as a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loc {
    /// A real folder on a disk.
    Dir(PathBuf),
    /// A place inside an archive file.
    Archive(crate::archive::Inside),
    /// The Windows Recycle Bin.
    Recycle,
    /// "This PC": the drives and the user folders.
    ThisPc,
}

impl Loc {
    /// Classifies a path. The special forms are recognised by the same magic
    /// paths the sidebar opens, so the two always agree.
    pub fn of(path: &Path) -> Loc {
        if crate::recycle::is_root(path) {
            return Loc::Recycle;
        }
        if crate::this_pc::is_root(path) {
            return Loc::ThisPc;
        }
        if let Some(inside) = crate::archive::split(path) {
            return Loc::Archive(inside);
        }
        Loc::Dir(path.to_path_buf())
    }
}

/// A place that can be listed one level deep.
///
/// The trait exists so a new kind of place (a shell namespace, a remote) is a
/// new implementation rather than another branch in [`read_dir`].
pub trait Backend {
    /// The entries directly inside this place.
    fn list(&self, show_hidden: bool) -> io::Result<Vec<Entry>>;
}

impl Backend for Loc {
    fn list(&self, show_hidden: bool) -> io::Result<Vec<Entry>> {
        match self {
            // The Recycle Bin has no folder behind it; its listing is built from
            // the deleted items' records instead.
            Loc::Recycle => Ok(crate::recycle::list()),
            // "This PC" is the same: its listing is the drives and user folders.
            Loc::ThisPc => Ok(crate::this_pc::list()),
            // An archive, or a place inside one, is read out of the archive.
            Loc::Archive(inside) => {
                let mut entries = crate::archive::list(&inside.archive, &inside.inner)?;
                if !show_hidden {
                    entries.retain(|e| !e.hidden);
                }
                Ok(entries)
            }
            Loc::Dir(path) => read_dir_fs(path, show_hidden),
        }
    }
}

/// Reads a directory into entries, skipping hidden files unless asked.
///
/// Runs on a worker thread; touches the disk exactly once.
pub fn read_dir(path: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    Backend::list(&Loc::of(path), show_hidden)
}

/// The filesystem half of [`Backend`].
fn read_dir_fs(path: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::with_capacity(64);
    // The OS is asked for the verbatim form, which reaches past `MAX_PATH`; the
    // entries keep the ordinary path, so everything above this (breadcrumbs,
    // comparisons, the address bar) sees the path the user typed.
    for item in fs::read_dir(long_path(path))? {
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
            path: path.join(name),
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

/// The user folders shown in the sidebar and on "This PC": Home, Desktop,
/// Documents, Downloads, Pictures, Music and Videos, in display order.
///
/// The Recycle Bin is deliberately not here: it is a place of its own in the
/// sidebar rather than a folder under This PC, and the magic paths are added by
/// [`places`].
pub fn user_folders() -> Vec<Place> {
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

/// User folders and mounted drives, in display order.
pub fn places() -> Vec<Place> {
    // This PC leads: it is the top of the hierarchy the user folders sit in,
    // and clicking it opens the view that gathers them with the drives.
    let mut out = vec![Place {
        label: String::from("This PC"),
        path: PathBuf::from(crate::this_pc::ROOT),
        kind: PlaceKind::Directory,
    }];
    out.extend(user_folders());
    // The Recycle Bin is a place of its own, at the bottom of the user folders.
    // It is Windows-only: the magic path lists as empty elsewhere.
    #[cfg(windows)]
    out.push(Place {
        label: String::from("Recycle Bin"),
        path: PathBuf::from(crate::recycle::ROOT),
        kind: PlaceKind::Directory,
    });
    out
}

/// How long the drive list is trusted.
const DRIVES_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// The last reading of the volumes: when it was taken, and the list.
type DrivesSnapshot = (std::time::Instant, Vec<Place>);

/// The last reading, shared by every caller.
///
/// Enumerating volumes opens each one and asks for its capacity, and on a
/// machine with a network share or a sleeping disk one call can take a long
/// time. The sidebar, This PC and the Recycle Bin all want the same list, so it
/// is read once per TTL however many of them ask.
fn drives_cache() -> &'static std::sync::Mutex<Option<DrivesSnapshot>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<DrivesSnapshot>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Mounted volumes, from a reading at most [`DRIVES_TTL`] old.
pub fn drives() -> Vec<Place> {
    if let Ok(g) = drives_cache().lock()
        && let Some((at, list)) = &*g
        && at.elapsed() < DRIVES_TTL
    {
        return list.clone();
    }
    let fresh = read_drives();
    if let Ok(mut g) = drives_cache().lock() {
        *g = Some((std::time::Instant::now(), fresh.clone()));
    }
    fresh
}

/// Reads the volumes now, touching the OS.
fn read_drives() -> Vec<Place> {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let mut out = Vec::new();
    for d in disks.list() {
        let mount = d.mount_point();
        // Skip pseudo / duplicate mounts so the list stays short and useful.
        let name = d.name().to_string_lossy().to_string();
        if out.iter().any(|p: &Place| p.path == mount) {
            continue;
        }
        out.push(Place {
            label: drive_label(mount, &name),
            path: mount.to_path_buf(),
            kind: PlaceKind::Drive,
        });
    }
    out
}

/// The label for a drive row.
///
/// On Windows `Disk::name()` is the volume's own label ("Windows", "Data"),
/// which on its own leaves the row with no drive letter to identify it. Show
/// the letter too, the way Explorer does - "Windows (C:)" - or just the letter
/// when the volume is unlabelled. Other platforms have no letters, so their
/// volume name is used as-is, falling back to the mount point.
fn drive_label(mount: &Path, name: &str) -> String {
    let letter = drive_letter(mount);
    match (letter, name.is_empty()) {
        (Some(letter), false) => format!("{name} ({letter})"),
        (Some(letter), true) => letter,
        (None, false) => name.to_owned(),
        (None, true) => mount.display().to_string(),
    }
}

/// The drive letter of a Windows mount point (`C:` or `C:\`), if it has one.
fn drive_letter(mount: &Path) -> Option<String> {
    let s = mount.to_string_lossy();
    let s = s.trim_end_matches(['\\', '/']);
    let bytes = s.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some(s.to_owned())
    } else {
        None
    }
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
    /// slow anyway - never for the thread that draws.
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

/// The sidebar's top-level entries - places and drives - as `(label, path,
/// is_device)`, kept so they are not asked of the OS on every frame.
///
/// The drive list comes from the OS and that can be slow, so a worker reads it
/// on a timer and a frame only ever copies what is already here. The first frame
/// shows the user folders, which are cheap, and leaves the drives to that worker
/// rather than stalling before the window paints.
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
            // The very first frame shows the user folders, which are cheap, and
            // leaves the drives to the worker: enumerating volumes asks the OS
            // about every one of them, and a network share or a sleeping disk can
            // take a long time. Waiting for it here is a stall before the window
            // paints, so the drives arrive a frame or two later instead.
            self.list = places()
                .into_iter()
                .map(|p| {
                    let device = p.is_device();
                    (p.label, p.path, device)
                })
                .collect();
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
    // The Recycle Bin has no folder segments; its magic path would otherwise
    // show as the literal `::Recycle::` in the address bar.
    if crate::recycle::is_root(path) {
        return vec![(String::from("Recycle Bin"), path.to_path_buf())];
    }
    // "This PC" is the same: one segment, named rather than spelled out.
    if crate::this_pc::is_root(path) {
        return vec![(String::from("This PC"), path.to_path_buf())];
    }
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
            // comes from the path so far - which is `C:\`, so it renders as `C`
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

/// The verbatim (`\\?\`) form of an absolute path, which lifts Windows'
/// `MAX_PATH` limit of 260 characters.
///
/// A verbatim path is handed to the OS unparsed, so it is cleaned lexically
/// first: without that, a `.` or `..` in it would be taken literally. A UNC
/// path takes the `\\?\UNC\` form rather than a plain prefix. A relative path,
/// one that is already verbatim, and every path off Windows are returned
/// unchanged, so the helper is a no-op wherever the limit does not exist.
pub fn long_path(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if p.is_relative() {
            return p.to_path_buf();
        }
        if p.to_string_lossy().starts_with(r"\\?\") {
            // Already verbatim: leave it exactly as it is, so a path handed to
            // us in that form is not quietly rewritten.
            return p.to_path_buf();
        }
        let cleaned = normalize(p);
        let s = cleaned.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\") {
            PathBuf::from(format!(r"\\?\UNC\{rest}"))
        } else {
            PathBuf::from(format!(r"\\?\{s}"))
        }
    }
    #[cfg(not(windows))]
    {
        p.to_path_buf()
    }
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

/// Expands one batch-rename pattern into the name an item at `index` gets.
///
/// The tokens are deliberately few and literal, so what the dialog previews is
/// exactly what a rename does:
///
/// * `{name}` - the original stem, everything before the last dot.
/// * `{ext}`  - the extension without its dot, empty when there is none.
/// * `{n}`    - the counter, starting wherever the dialog's start number says.
/// * `{n:3}`  - the same counter padded with leading zeros to three digits.
///
/// Anything else, including an unclosed brace, is left exactly as typed: a
/// pattern is the user's text, and quietly dropping part of it would rename
/// files to something they never asked for.
pub fn batch_name(pattern: &str, stem: &str, ext: &str, index: usize) -> String {
    let mut out = String::with_capacity(pattern.len() + stem.len() + 8);
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let token = &after[..close];
                match expand_token(token, stem, ext, index) {
                    Some(s) => out.push_str(&s),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                // No closing brace at all: the remainder is literal.
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// One `{...}` token of a batch-rename pattern, or `None` when it is not a
/// token this understands and should be kept verbatim.
fn expand_token(token: &str, stem: &str, ext: &str, index: usize) -> Option<String> {
    match token {
        "name" => Some(stem.to_owned()),
        "ext" => Some(ext.to_owned()),
        "n" => Some(index.to_string()),
        _ => {
            let digits: usize = token.strip_prefix("n:")?.parse().ok()?;
            Some(format!("{index:0digits$}"))
        }
    }
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
        ("this_pc::list()", || drop(crate::this_pc::list())),
        ("recycle::list()", || drop(crate::recycle::list())),
    ] {
        let mut times = Vec::new();
        for _ in 0..10 {
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
        // The characters Windows forbids are legal on Unix, and the check
        // follows the platform it runs on.
        #[cfg(windows)]
        {
            assert!(validate_name("x*").is_err());
            assert!(validate_name("CON").is_err());
        }
        #[cfg(not(windows))]
        {
            assert!(validate_name("x*").is_ok());
            assert!(validate_name("CON").is_ok());
        }
    }

    #[test]
    fn batch_pattern_expands_every_token() {
        assert_eq!(
            batch_name("{name}.{ext}", "holiday", "jpg", 1),
            "holiday.jpg"
        );
        assert_eq!(
            batch_name("{name} - copy", "notes", "txt", 1),
            "notes - copy"
        );
        // The counter starts where the caller says and pads as asked.
        assert_eq!(batch_name("photo_{n}", "x", "png", 7), "photo_7");
        assert_eq!(batch_name("img_{n:3}.{ext}", "x", "png", 4), "img_004.png");
        assert_eq!(batch_name("{n:2}", "x", "", 12), "12");
    }

    #[test]
    fn batch_pattern_keeps_a_missing_extension_empty() {
        // `{ext}` with nothing to expand to is empty, not a stray dot of its own.
        assert_eq!(batch_name("{name}{ext}", "readme", "", 1), "readme");
        assert_eq!(batch_name("{name}.{ext}", "readme", "", 1), "readme.");
        assert_eq!(batch_name("{name}_{n}", "readme", "", 3), "readme_3");
    }

    #[test]
    fn batch_pattern_leaves_unknown_tokens_alone() {
        assert_eq!(batch_name("{name}-{who}", "a", "txt", 1), "a-{who}");
        assert_eq!(batch_name("{name", "a", "txt", 1), "{name");
        assert_eq!(batch_name("plain", "a", "txt", 1), "plain");
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
        // the root as well - which is derived from the accumulated path and so comes
        // out as "C" - puts the drive letter in the address bar twice, as
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
    fn this_pc_breadcrumb_shows_its_name_and_not_the_magic_string() {
        let root = Path::new(crate::this_pc::ROOT);
        let bc = breadcrumbs(root);
        assert_eq!(bc.len(), 1, "This PC is one segment: {bc:?}");
        assert_eq!(bc[0].0, "This PC");
        // The segment still leads to the place itself, so a click stays here.
        assert_eq!(bc[0].1, PathBuf::from(crate::this_pc::ROOT));
        assert!(
            !bc.iter().any(|(label, _)| label.contains("::")),
            "the magic string is shown: {bc:?}"
        );
    }

    #[test]
    fn read_dir_of_the_this_pc_root_lists_the_drives_and_user_folders() {
        let entries = read_dir(Path::new(crate::this_pc::ROOT), false).unwrap();
        let expected = crate::this_pc::list();
        assert_eq!(entries.len(), expected.len(), "{entries:?}");
        for (got, want) in entries.iter().zip(expected.iter()) {
            assert_eq!(got.name, want.name);
            assert_eq!(got.path, want.path);
            assert!(got.is_dir, "{got:?}");
        }
        assert!(entries.iter().any(|e| e.name == "Home"), "{entries:?}");
    }

    #[test]
    fn drive_rows_carry_their_letter() {
        // A labelled Windows volume reads like Explorer's "Windows (C:)".
        assert_eq!(drive_label(Path::new("C:"), "Windows"), "Windows (C:)");
        assert_eq!(drive_label(Path::new(r"C:\"), "Windows"), "Windows (C:)");
        // An unlabelled one is just the letter.
        assert_eq!(drive_label(Path::new(r"D:\"), ""), "D:");
        // A mount that is not a bare letter is left alone, name only.
        assert_eq!(drive_label(Path::new(r"C:\Mounts\Data"), "Data"), "Data");
        // Other platforms have no letters: the volume name is the label.
        assert_eq!(drive_label(Path::new("/"), "Macintosh HD"), "Macintosh HD");
        assert_eq!(drive_label(Path::new("/"), ""), "/");
    }

    #[test]
    fn normalize_resolves_dot_segments() {
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn long_path_prefixes_a_drive_path() {
        if !cfg!(windows) {
            return;
        }
        assert_eq!(
            long_path(Path::new(r"C:\a\b")),
            PathBuf::from(r"\\?\C:\a\b")
        );
        // The `.` and `..` are cleaned before the prefix goes on: a verbatim path
        // is handed to the OS unparsed, so it would otherwise be read literally.
        assert_eq!(
            long_path(Path::new(r"C:\a\..\b")),
            PathBuf::from(r"\\?\C:\b")
        );
    }

    #[test]
    fn long_path_prefixes_a_unc_path() {
        if !cfg!(windows) {
            return;
        }
        assert_eq!(
            long_path(Path::new(r"\\server\share\a")),
            PathBuf::from(r"\\?\UNC\server\share\a")
        );
    }

    #[test]
    fn long_path_leaves_relative_and_verbatim_paths_alone() {
        // A relative path is left exactly as it was given, on every platform.
        assert_eq!(long_path(Path::new("a/b")), PathBuf::from("a/b"));
        assert_eq!(long_path(Path::new("/a/b")), PathBuf::from("/a/b"));
        if cfg!(windows) {
            let verbatim = PathBuf::from(r"\\?\C:\a\b");
            assert_eq!(long_path(&verbatim), verbatim);
            let unc = PathBuf::from(r"\\?\UNC\server\share\a");
            assert_eq!(long_path(&unc), unc);
        }
    }

    /// A path under `root` whose full length clears `min`, so it is past the old
    /// 260-character limit.
    fn deeper_than(root: &Path, min: usize) -> PathBuf {
        let segment = "0123456789abcdefghijklmnopqrstuvwxyz";
        let mut p = root.to_path_buf();
        while p.as_os_str().len() <= min {
            p.push(segment);
        }
        p
    }

    #[test]
    fn a_path_beyond_max_path_round_trips_through_the_helper() {
        if !cfg!(windows) {
            return;
        }
        let root = std::env::temp_dir().join("rhumb-long-path");
        let _ = fs::remove_dir_all(long_path(&root));
        let deep = deeper_than(&root, 300);
        assert!(
            deep.as_os_str().len() > 260,
            "the test tree is not long enough: {}",
            deep.as_os_str().len()
        );

        // Create, write and read back, all through the verbatim form.
        fs::create_dir_all(long_path(&deep)).unwrap();
        let file = deep.join("hello.txt");
        fs::write(long_path(&file), b"deep").unwrap();
        assert_eq!(fs::read(long_path(&file)).unwrap(), b"deep");

        // Listing goes through the app's own door to the OS and comes back with
        // ordinary paths, not the verbatim ones.
        let entries = read_dir(&deep, false).unwrap();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].name, "hello.txt");
        assert_eq!(entries[0].path, file, "the entry keeps the plain path");
        assert!(!entries[0].is_dir);

        fs::remove_dir_all(long_path(&root)).unwrap();
        assert!(fs::metadata(long_path(&deep)).is_err(), "the tree survived");
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

    /// One entry for the grouping and filtering tests, with only the fields
    /// those read filled in.
    fn entry(name: &str, is_dir: bool, size: u64, modified: Option<SystemTime>) -> Entry {
        Entry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            size,
            modified,
            hidden: false,
        }
    }

    fn now() -> SystemTime {
        SystemTime::now()
    }

    #[test]
    fn names_bucket_into_a_to_z_and_the_rest_under_hash() {
        let cases = [
            ("apple.txt", "A"),
            ("Banana.txt", "B"),
            ("cherry", "C"),
            ("zebra.zip", "Z"),
            ("_hidden", "#"),
            ("123.txt", "#"),
            (".config", "#"),
        ];
        for (name, want) in cases {
            let e = entry(name, false, 0, None);
            assert_eq!(
                group_label(&e, GroupBy::Name),
                want,
                "{name} landed in the wrong bucket"
            );
        }
        // The buckets sort # first, then A..Z.
        assert!(group_rank(GroupBy::Name, "#") < group_rank(GroupBy::Name, "A"));
        assert!(group_rank(GroupBy::Name, "A") < group_rank(GroupBy::Name, "Z"));
    }

    #[test]
    fn kind_classifies_extensions_into_their_families() {
        assert_eq!(kind_of("txt"), FileKind::Document);
        assert_eq!(kind_of("PNG"), FileKind::Image);
        assert_eq!(kind_of("mp3"), FileKind::Audio);
        assert_eq!(kind_of("mkv"), FileKind::Video);
        assert_eq!(kind_of("zip"), FileKind::Archive);
        assert_eq!(kind_of("rs"), FileKind::Other);
        assert_eq!(kind_of(""), FileKind::Other);
    }

    #[test]
    fn type_grouping_puts_folders_first_then_the_families() {
        let cases = [
            (entry("docs", true, 0, None), "Folders"),
            (entry("a.txt", false, 0, None), "Documents"),
            (entry("b.png", false, 0, None), "Images"),
            (entry("c.mp3", false, 0, None), "Audio"),
            (entry("d.mp4", false, 0, None), "Video"),
            (entry("e.zip", false, 0, None), "Archives"),
            (entry("f.rs", false, 0, None), "Other"),
        ];
        for (e, want) in &cases {
            assert_eq!(&group_label(e, GroupBy::Type), want, "{}", e.name);
        }
        let mut ranks: Vec<usize> = cases
            .iter()
            .map(|(_, want)| group_rank(GroupBy::Type, want))
            .collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(ranks.len(), cases.len(), "the type groups are distinct");
        assert!(group_rank(GroupBy::Type, "Folders") < group_rank(GroupBy::Type, "Documents"));
    }

    #[test]
    fn the_place_grouping_puts_volumes_after_the_folders() {
        let cases = [
            (entry("C:\\", true, 0, None), "Devices and drives"),
            (entry("docs", true, 0, None), "Folders"),
        ];
        for (e, want) in &cases {
            assert_eq!(&group_label(e, GroupBy::Place), want, "{}", e.name);
        }
        assert!(
            group_rank(GroupBy::Place, "Folders")
                < group_rank(GroupBy::Place, "Devices and drives")
        );
    }

    #[test]
    fn a_size_lands_in_the_band_the_boundary_says() {
        let cases = [
            (0u64, "Small"),
            (MB - 1, "Small"),
            (MB, "Medium"),
            (100 * MB - 1, "Medium"),
            (100 * MB, "Large"),
            (u64::MAX, "Large"),
        ];
        for (size, want) in cases {
            let e = entry("f.bin", false, size, None);
            assert_eq!(group_label(&e, GroupBy::Size), want, "size {size}");
        }
        // A folder is its own band, whatever its own byte count is.
        let folder = entry("dir", true, 100 * MB, None);
        assert_eq!(group_label(&folder, GroupBy::Size), "Folders");
    }

    #[test]
    fn a_date_lands_in_the_bucket_the_boundary_says() {
        // The boundaries themselves, without depending on the wall clock.
        assert_eq!(bucket_days(-1), "Today", "a future date reads as today");
        assert_eq!(bucket_days(0), "Today");
        assert_eq!(bucket_days(1), "Yesterday");
        assert_eq!(bucket_days(2), "Earlier this week");
        assert_eq!(bucket_days(6), "Earlier this week");
        assert_eq!(bucket_days(7), "Earlier this month");
        assert_eq!(bucket_days(29), "Earlier this month");
        assert_eq!(bucket_days(30), "A long time ago");
        assert_eq!(bucket_days(i64::MAX), "A long time ago");
    }

    #[test]
    fn days_ago_counts_whole_local_days() {
        let t = now();
        assert_eq!(days_ago(Some(t), t), Some(0));
        assert_eq!(
            days_ago(Some(t - std::time::Duration::from_secs(10 * 86_400)), t),
            Some(10)
        );
        assert_eq!(days_ago(None, t), None);
    }

    #[test]
    fn kind_filter_keeps_only_what_it_names() {
        let dir = entry("dir", true, 0, None);
        let doc = entry("a.txt", false, 0, None);
        let img = entry("b.png", false, 0, None);
        let arc = entry("c.zip", false, 0, None);
        assert!(KindFilter::All.accepts(&dir) && KindFilter::All.accepts(&doc));
        assert!(KindFilter::Folders.accepts(&dir) && !KindFilter::Folders.accepts(&doc));
        assert!(!KindFilter::Files.accepts(&dir) && KindFilter::Files.accepts(&doc));
        assert!(KindFilter::Documents.accepts(&doc) && !KindFilter::Documents.accepts(&img));
        assert!(KindFilter::Images.accepts(&img) && !KindFilter::Images.accepts(&doc));
        assert!(KindFilter::Archives.accepts(&arc) && !KindFilter::Archives.accepts(&doc));
        // A folder never matches an extension choice, however it is named.
        let folder_txt = entry("folder.txt", true, 0, None);
        assert!(!KindFilter::Documents.accepts(&folder_txt));
    }

    #[test]
    fn date_filter_keeps_only_what_is_recent_enough() {
        let t = now();
        let at = |days: u64| {
            entry(
                "f",
                false,
                0,
                Some(t - std::time::Duration::from_secs(days * 86_400)),
            )
        };
        assert!(DateFilter::Any.accepts(&entry("f", false, 0, None), t));
        assert!(DateFilter::Today.accepts(&at(0), t));
        assert!(!DateFilter::Today.accepts(&at(2), t));
        assert!(DateFilter::Last7.accepts(&at(3), t));
        assert!(!DateFilter::Last7.accepts(&at(10), t));
        assert!(DateFilter::Last30.accepts(&at(10), t));
        assert!(!DateFilter::Last30.accepts(&at(60), t));
        assert!(DateFilter::ThisYear.accepts(&at(0), t));
        // Two years back is safely in an earlier calendar year, whatever today is.
        assert!(!DateFilter::ThisYear.accepts(&at(800), t));
        // An entry with no date fails every dated choice but Any.
        let undated = entry("f", false, 0, None);
        assert!(!DateFilter::Today.accepts(&undated, t));
        assert!(!DateFilter::ThisYear.accepts(&undated, t));
    }

    #[test]
    fn size_filter_bands_and_exempts_folders() {
        let small = entry("s.bin", false, MB - 1, None);
        let medium = entry("m.bin", false, 10 * MB, None);
        let large = entry("l.bin", false, 200 * MB, None);
        assert!(SizeFilter::Any.accepts(&small));
        assert!(SizeFilter::Small.accepts(&small) && !SizeFilter::Small.accepts(&medium));
        assert!(SizeFilter::Medium.accepts(&medium) && !SizeFilter::Medium.accepts(&large));
        assert!(SizeFilter::Large.accepts(&large) && !SizeFilter::Large.accepts(&medium));
        // A folder is never hidden by the size filter.
        let folder = entry("dir", true, 0, None);
        for f in SizeFilter::ALL {
            assert!(f.accepts(&folder), "{f:?} hid a folder");
        }
    }

    #[test]
    fn every_choice_has_a_key_that_reads_back() {
        for g in GroupBy::ALL {
            assert_eq!(GroupBy::from_key(g.key()), g, "{g:?}");
        }
        for k in KindFilter::ALL {
            assert_eq!(KindFilter::from_key(k.key()), k, "{k:?}");
        }
        for d in DateFilter::ALL {
            assert_eq!(DateFilter::from_key(d.key()), d, "{d:?}");
        }
        for s in SizeFilter::ALL {
            assert_eq!(SizeFilter::from_key(s.key()), s, "{s:?}");
        }
    }
}
