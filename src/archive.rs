//! Archives as folders.
//!
//! A `.zip`, `.tar`, `.tar.gz`, `.7z` or `.rar` can be opened like a folder: its path followed
//! by a path inside it, `C:\files\photos.zip\2024\beach.jpg`, names a place the file
//! list can show, the address bar can say and the history can come back to. Nothing on the
//! disk has such a path, so this module is the one place that knows how to read it:
//! [`split`] finds where the archive stops and the path inside starts, [`list`] reads
//! one level of what is inside, and [`extract_to`] and [`materialize`] bring files out
//! to somewhere real, which is what opening, copying and dragging need.
//!
//! Archives are read-only except for a zip, which can be written into the way
//! Explorer edits a compressed folder: [`add_to_zip`] appends files and
//! [`remove_from_zip`] rewrites the archive without the names asked for. Every
//! other format stays read-only.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::fs_model::{Entry, long_path};

/// The archive formats that can be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Zip,
    Tar,
    TarGz,
    SevenZ,
    Rar,
}

/// What a name says about being an archive, by its extension.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".zip") || name.ends_with(".jar") {
        Some(Kind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Kind::TarGz)
    } else if name.ends_with(".tar") {
        Some(Kind::Tar)
    } else if name.ends_with(".7z") {
        Some(Kind::SevenZ)
    } else if name.ends_with(".rar") {
        Some(Kind::Rar)
    } else {
        None
    }
}

/// Whether `path` is an archive file on the disk, which is a place that can be opened.
pub fn is_archive_file(path: &Path) -> bool {
    kind_of(path).is_some() && path.is_file()
}

/// An archive and a place inside it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Inside {
    /// The archive file, a real path.
    pub archive: PathBuf,
    /// Where in it, with `/` between the names and none at the ends. Empty is the top.
    pub inner: String,
}

/// Finds the archive a path leads into, if it does: the first part of the path that
/// is an archive file on the disk, and what follows it.
pub fn split(path: &Path) -> Option<Inside> {
    let comps: Vec<Component> = path.components().collect();
    let mut prefix = PathBuf::new();
    for (i, c) in comps.iter().enumerate() {
        prefix.push(c.as_os_str());
        if kind_of(&prefix).is_some() && prefix.is_file() {
            let inner = comps[i + 1..]
                .iter()
                .filter_map(|c| match c {
                    Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            return Some(Inside {
                archive: prefix,
                inner,
            });
        }
    }
    None
}

/// Whether the path is inside an archive, or is one, so the disk cannot answer for it.
pub fn is_virtual(path: &Path) -> bool {
    split(path).is_some()
}

/// The path of a place inside an archive.
pub fn join(archive: &Path, inner: &str) -> PathBuf {
    let mut p = archive.to_path_buf();
    for seg in inner.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

/// One name read from an archive.
#[derive(Clone, Debug)]
struct Item {
    /// The path inside, `/` between the names, none at the ends.
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

/// Every name in one archive, kept until the archive changes.
struct Table {
    len: u64,
    modified: Option<SystemTime>,
    items: Vec<Item>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Arc<Table>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Table>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn invalid(msg: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// A name as stored, made safe and tidy: slashes of one kind, no empty or `.` parts,
/// and `None` for anything that climbs out with `..` or starts from a drive or a root.
fn clean(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for (i, part) in name.split('/').enumerate() {
        match part {
            "" | "." => {}
            ".." => return None,
            // `C:` as the first part is a drive, which names somewhere outside.
            p if i == 0 && p.len() == 2 && p.ends_with(':') => return None,
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn table(archive: &Path) -> io::Result<Arc<Table>> {
    let meta = std::fs::metadata(long_path(archive))?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    if let Some(t) = cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(archive)
        && t.len == len
        && t.modified == modified
    {
        return Ok(t.clone());
    }
    let kind = kind_of(archive).ok_or_else(|| invalid("not an archive"))?;
    let items = match kind {
        Kind::Zip => read_zip(archive)?,
        Kind::Tar => read_tar(BufReader::new(File::open(long_path(archive))?))?,
        Kind::TarGz => read_tar(flate2::read::GzDecoder::new(BufReader::new(File::open(
            archive,
        )?)))?,
        Kind::SevenZ => read_7z(archive)?,
        Kind::Rar => read_rar(archive)?,
    };
    let t = Arc::new(Table {
        len,
        modified,
        items,
    });
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    // Bounded: a window that has looked in a great many archives does not keep every
    // table for good.
    if c.len() >= 16 {
        c.clear();
    }
    c.insert(archive.to_path_buf(), t.clone());
    Ok(t)
}

fn zip_time(d: zip::DateTime) -> Option<SystemTime> {
    // Days from 1970-01-01 to the date, by the civil calendar rule.
    let (y, m, day) = (
        i64::from(d.year()),
        i64::from(d.month()),
        i64::from(d.day()),
    );
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400
        + i64::from(d.hour()) * 3600
        + i64::from(d.minute()) * 60
        + i64::from(d.second());
    u64::try_from(secs)
        .ok()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

fn read_zip(archive: &Path) -> io::Result<Vec<Item>> {
    let mut z =
        zip::ZipArchive::new(BufReader::new(File::open(long_path(archive))?)).map_err(invalid)?;
    let mut items = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        // The raw entry: its header is all that is wanted, and an encrypted file has
        // one like any other.
        let f = z.by_index_raw(i).map_err(invalid)?;
        let Some(path) = clean(f.name()) else {
            continue;
        };
        items.push(Item {
            path,
            is_dir: f.is_dir() || f.name().ends_with('/'),
            size: f.size(),
            modified: f.last_modified().and_then(zip_time),
        });
    }
    Ok(items)
}

fn read_tar<R: Read>(reader: R) -> io::Result<Vec<Item>> {
    let mut ar = tar::Archive::new(reader);
    let mut items = Vec::new();
    for entry in ar.entries()? {
        let entry = entry?;
        let raw = entry.path()?.to_string_lossy().into_owned();
        let Some(path) = clean(&raw) else {
            continue;
        };
        let t = entry.header().entry_type();
        // Links and device files are not shown: there is nothing in them to read.
        if !(t.is_file() || t.is_dir() || t.is_contiguous()) {
            continue;
        }
        items.push(Item {
            path,
            is_dir: t.is_dir(),
            size: entry.header().size().unwrap_or(0),
            modified: entry
                .header()
                .mtime()
                .ok()
                .map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        });
    }
    Ok(items)
}

/// A 7z time as a `SystemTime`, if the archive recorded one. Kept clear of
/// `SystemTime::from`, which panics on a time it cannot hold; a corrupt archive
/// must be an error, not a crash.
fn sevenz_time(e: &sevenz_rust::SevenZArchiveEntry) -> Option<SystemTime> {
    if !e.has_last_modified_date {
        return None;
    }
    let secs = e.last_modified_date().to_unix_time();
    let d = Duration::from_secs(secs.unsigned_abs());
    if secs >= 0 {
        UNIX_EPOCH.checked_add(d)
    } else {
        UNIX_EPOCH.checked_sub(d)
    }
}

fn read_7z(archive: &Path) -> io::Result<Vec<Item>> {
    // Only the header is read here; the file data stays packed until something
    // is extracted. A solid block shares one decoder, so listing must not decode.
    let sz = sevenz_rust::SevenZReader::open(long_path(archive), sevenz_rust::Password::empty())
        .map_err(invalid)?;
    let mut items = Vec::with_capacity(sz.archive().files.len());
    for e in &sz.archive().files {
        let Some(path) = clean(e.name()) else {
            continue;
        };
        items.push(Item {
            path,
            is_dir: e.is_directory(),
            size: e.size(),
            modified: sevenz_time(e),
        });
    }
    Ok(items)
}

/// A RAR time as a `SystemTime`, if the archive recorded one. RAR 5 stores Unix
/// seconds while older RAR stores DOS wall-clock fields; the crate knows which
/// and its own conversion is range-checked, but a corrupt archive must be an
/// error and not a crash, so nothing here is allowed to overflow.
fn rar_time(m: &rars::ArchiveMemberMeta) -> Option<SystemTime> {
    let raw = m.file_time?;
    if m.family == rars::ArchiveFamily::Rar50Plus {
        // The refinement is documented to sit below one second; clamp anyway so
        // a malformed value cannot overflow `Duration`.
        let nanos = m
            .mtime_refinement
            .map_or(0, |r| r.nanoseconds.min(999_999_999));
        return UNIX_EPOCH.checked_add(Duration::new(u64::from(raw), nanos));
    }
    // Older families refuse impossible DOS fields themselves, returning `None`.
    m.modification_time()
}

fn read_rar(archive: &Path) -> io::Result<Vec<Item>> {
    // The archive stays an open file rather than being read into memory: only
    // the headers are wanted to list it, and the payloads are read on extraction.
    let ar = rars::ArchiveReader::read_reader(File::open(long_path(archive))?).map_err(invalid)?;
    let mut items = Vec::new();
    for m in ar.members() {
        let Some(path) = clean(&m.meta.name_lossy()) else {
            continue;
        };
        items.push(Item {
            path,
            is_dir: m.meta.is_directory,
            size: m.meta.unpacked_size,
            modified: rar_time(&m.meta),
        });
    }
    Ok(items)
}

/// One level of an archive: what is directly inside `inner`. Folders that the archive
/// only implies, by having a file inside them, are made up.
pub fn list(archive: &Path, inner: &str) -> io::Result<Vec<Entry>> {
    let table = table(archive)?;
    let inner = clean(inner).unwrap_or_default();
    let prefix = if inner.is_empty() {
        String::new()
    } else {
        format!("{inner}/")
    };
    let mut found: BTreeMap<String, Entry> = BTreeMap::new();
    let mut inside = inner.is_empty();
    for item in &table.items {
        let Some(rest) = item.path.strip_prefix(&prefix) else {
            // The folder itself, named by its own entry.
            if item.path == inner {
                inside = true;
            }
            continue;
        };
        inside = true;
        let (name, deeper) = match rest.split_once('/') {
            Some((first, _)) => (first, true),
            None => (rest, false),
        };
        if name.is_empty() {
            continue;
        }
        let is_dir = deeper || item.is_dir;
        let entry = found.entry(name.to_owned()).or_insert_with(|| Entry {
            name: name.to_owned(),
            path: join(archive, &format!("{prefix}{name}")),
            is_dir,
            is_symlink: false,
            size: 0,
            modified: None,
            hidden: name.starts_with('.'),
        });
        if is_dir {
            entry.is_dir = true;
        }
        if !deeper && !item.is_dir {
            entry.size = item.size;
            entry.modified = item.modified;
        } else if !deeper && entry.modified.is_none() {
            entry.modified = item.modified;
        }
    }
    if !inside {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{inner} is not in the archive"),
        ));
    }
    Ok(found.into_values().collect())
}

/// Whether a place inside an archive is a folder.
pub fn is_dir_inside(archive: &Path, inner: &str) -> bool {
    let Ok(table) = table(archive) else {
        return false;
    };
    let inner = clean(inner).unwrap_or_default();
    if inner.is_empty() {
        return true;
    }
    let prefix = format!("{inner}/");
    table
        .items
        .iter()
        .any(|i| (i.path == inner && i.is_dir) || i.path.starts_with(&prefix))
}

/// What one extraction brought out.
#[derive(Debug)]
pub struct Extracted {
    /// How many files were written.
    pub files: usize,
    /// Where the thing asked for ended up: the file, or the folder.
    pub at: PathBuf,
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

/// How many files, and how many bytes, an extraction of `inner` would write.
pub fn count_files(archive: &Path, inner: &str) -> io::Result<(usize, u64)> {
    let table = table(archive)?;
    let inner = clean(inner).unwrap_or_default();
    let prefix = format!("{inner}/");
    let (mut n, mut bytes) = (0usize, 0u64);
    for i in &table.items {
        if i.is_dir {
            continue;
        }
        if inner.is_empty() || i.path == inner || i.path.starts_with(&prefix) {
            n += 1;
            bytes += i.size;
        }
    }
    Ok((n, bytes))
}

/// A name from an archive as a path under `dest`, or `None` if it would land anywhere
/// else. Names are cleaned already, so this is the last line of defence.
fn under(dest: &Path, name: &str) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    for seg in name.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains(':') {
            return None;
        }
        out.push(seg);
    }
    out.starts_with(dest).then_some(out)
}

/// A short name for an error message, falling back to the whole path.
fn shown(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A temporary name beside `path`, on the same volume so the finished file can
/// replace the original with a rename. A counter keeps two edits of one archive
/// from choosing the same name.
fn temp_beside(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(
        ".{}.rhumb-{}-{n}.tmp",
        shown(path),
        std::process::id()
    ))
}

/// Adds `sources` to an existing zip, under `inner_dir` (empty is the top), the
/// way Explorer adds to a compressed folder. Every entry already in the archive
/// is copied across unchanged and the new files are appended; the result is
/// written beside the archive and only swapped in once it is whole, so a failure
/// leaves the original untouched.
///
/// Returns how many files were added. A folder contributes its files at every
/// depth, with `/` between the names, and keeps its own entry too, so an empty
/// folder survives. Only zip archives can be changed; a tar, 7z or rar is
/// refused, and so is a name that would climb out of the archive.
pub fn add_to_zip(zip: &Path, inner_dir: &str, sources: &[PathBuf]) -> Result<usize, String> {
    if kind_of(zip) != Some(Kind::Zip) {
        return Err(format!(
            "{} is not a zip archive: only zips can be changed",
            shown(zip)
        ));
    }
    let dir = if inner_dir.trim_matches(['/', '\\']).is_empty() {
        String::new()
    } else {
        clean(inner_dir).ok_or_else(|| format!("unsafe folder {inner_dir:?}"))?
    };
    // Work out every name before touching the archive, so a bad one is refused
    // with the archive still as it was.
    let mut new: Vec<(PathBuf, String, bool)> = Vec::new();
    for src in sources {
        let Some(base) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let top = if dir.is_empty() {
            base
        } else {
            format!("{dir}/{base}")
        };
        if src.is_dir() {
            new.push((src.clone(), top.clone(), true));
            for entry in walkdir::WalkDir::new(src).min_depth(1).sort_by_file_name() {
                let entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.file_type();
                // Links and device files have no bytes to store, so they are
                // left out rather than followed.
                if !(kind.is_file() || kind.is_dir()) {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(src)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                new.push((
                    entry.path().to_path_buf(),
                    format!("{top}/{rel}"),
                    kind.is_dir(),
                ));
            }
        } else {
            new.push((src.clone(), top, false));
        }
    }
    if new.is_empty() {
        return Ok(0);
    }
    // The same last line of defence the reader uses, so nothing is ever written
    // under a name that could climb out.
    for (_, name, _) in &new {
        if under(Path::new(""), name).is_none() {
            return Err(format!("unsafe name {name:?}"));
        }
    }

    let tmp = temp_beside(zip);
    let result = (|| -> Result<usize, String> {
        let mut reader = zip::ZipArchive::new(BufReader::new(
            File::open(long_path(zip)).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        let mut writer =
            zip::ZipWriter::new(File::create(long_path(&tmp)).map_err(|e| e.to_string())?);
        // Existing entries keep their bytes and their compression; only the
        // directory around them changes.
        for i in 0..reader.len() {
            let f = reader.by_index_raw(i).map_err(|e| e.to_string())?;
            writer.raw_copy_file(f).map_err(|e| e.to_string())?;
        }
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let mut files = 0usize;
        for (from, name, is_dir) in &new {
            if *is_dir {
                writer
                    .add_directory(name.clone(), options)
                    .map_err(|e| e.to_string())?;
            } else {
                writer
                    .start_file(name.clone(), options)
                    .map_err(|e| e.to_string())?;
                let mut r = File::open(long_path(from)).map_err(|e| e.to_string())?;
                io::copy(&mut r, &mut writer).map_err(|e| e.to_string())?;
                files += 1;
            }
        }
        writer.finish().map_err(|e| e.to_string())?;
        // Windows will not rename over a file that is still open, so the reader
        // is let go before the swap.
        drop(reader);
        std::fs::rename(long_path(&tmp), long_path(zip)).map_err(|e| e.to_string())?;
        Ok(files)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(long_path(&tmp));
    }
    result
}

/// Rewrites a zip without the named entries, each an inner path with `/` between
/// the names and none at the ends. Removing a folder removes everything under
/// it. The rewrite goes beside the archive and replaces it only on success, so a
/// failure leaves the original whole.
///
/// Returns how many entries were removed. Only zip archives can be changed.
pub fn remove_from_zip(zip: &Path, entries: &[String]) -> Result<usize, String> {
    if kind_of(zip) != Some(Kind::Zip) {
        return Err(format!(
            "{} is not a zip archive: only zips can be changed",
            shown(zip)
        ));
    }
    let targets: Vec<String> = entries.iter().filter_map(|e| clean(e)).collect();
    if targets.is_empty() {
        return Ok(0);
    }
    let tmp = temp_beside(zip);
    let result = (|| -> Result<usize, String> {
        let mut reader = zip::ZipArchive::new(BufReader::new(
            File::open(long_path(zip)).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        let mut writer =
            zip::ZipWriter::new(File::create(long_path(&tmp)).map_err(|e| e.to_string())?);
        let mut removed = 0usize;
        for i in 0..reader.len() {
            let f = reader.by_index_raw(i).map_err(|e| e.to_string())?;
            let gone = clean(f.name()).is_some_and(|n| {
                targets
                    .iter()
                    .any(|t| n == *t || n.starts_with(&format!("{t}/")))
            });
            if gone {
                removed += 1;
                continue;
            }
            writer.raw_copy_file(f).map_err(|e| e.to_string())?;
        }
        writer.finish().map_err(|e| e.to_string())?;
        drop(reader);
        std::fs::rename(long_path(&tmp), long_path(zip)).map_err(|e| e.to_string())?;
        Ok(removed)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(long_path(&tmp));
    }
    result
}

/// Writes what is at `inner` (a file, a folder with all it holds, or with `inner`
/// empty the whole archive) into `dest`, which is made if it is not there. A file or
/// folder lands as `dest/<its name>`; the whole archive lands as its contents. Names
/// that would climb out of `dest` are skipped.
pub fn extract_to(archive: &Path, inner: &str, dest: &Path) -> io::Result<Extracted> {
    extract_with(archive, inner, dest, &mut |_, _| true)
}

/// The same, calling `step` with the name and size of each file before it is written;
/// answering `false` stops the extraction there with an error of kind `Interrupted`.
pub fn extract_with(
    archive: &Path,
    inner: &str,
    dest: &Path,
    step: &mut dyn FnMut(&str, u64) -> bool,
) -> io::Result<Extracted> {
    let inner = clean(inner).unwrap_or_default();
    // What is taken off the front of each name, so a folder lands under its own name.
    let strip = match inner.rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/"),
        None => String::new(),
    };
    let wanted =
        |name: &str| inner.is_empty() || name == inner || name.starts_with(&format!("{inner}/"));
    std::fs::create_dir_all(long_path(dest))?;
    let kind = kind_of(archive).ok_or_else(|| invalid("not an archive"))?;
    let mut files = 0usize;
    let mut found = false;
    match kind {
        Kind::Zip => {
            let mut z = zip::ZipArchive::new(BufReader::new(File::open(long_path(archive))?))
                .map_err(invalid)?;
            for i in 0..z.len() {
                let mut f = z.by_index(i).map_err(invalid)?;
                let Some(name) = clean(f.name()) else {
                    continue;
                };
                if !wanted(&name) {
                    continue;
                }
                found = true;
                let rel = name.strip_prefix(&strip).unwrap_or(&name);
                let Some(out) = under(dest, rel) else {
                    continue;
                };
                if f.is_dir() || f.name().ends_with('/') {
                    std::fs::create_dir_all(long_path(&out))?;
                } else {
                    if !step(&name, f.size()) {
                        return Err(cancelled());
                    }
                    if let Some(parent) = out.parent() {
                        std::fs::create_dir_all(long_path(parent))?;
                    }
                    let mut w = File::create(long_path(&out))?;
                    io::copy(&mut f, &mut w)?;
                    w.flush()?;
                    files += 1;
                }
            }
        }
        Kind::Tar => extract_tar(
            BufReader::new(File::open(long_path(archive))?),
            &wanted,
            &strip,
            dest,
            &mut files,
            &mut found,
            step,
        )?,
        Kind::TarGz => extract_tar(
            flate2::read::GzDecoder::new(BufReader::new(File::open(long_path(archive))?)),
            &wanted,
            &strip,
            dest,
            &mut files,
            &mut found,
            step,
        )?,
        Kind::SevenZ => extract_7z(archive, &wanted, &strip, dest, &mut files, &mut found, step)?,
        Kind::Rar => extract_rar(archive, &wanted, &strip, dest, &mut files, &mut found, step)?,
    }
    if !found && !inner.is_empty() {
        // A folder the archive only implies has no entry of its own to have matched.
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{inner} is not in the archive"),
        ));
    }
    let at = if inner.is_empty() {
        dest.to_path_buf()
    } else {
        under(dest, inner.rsplit('/').next().unwrap_or(&inner))
            .unwrap_or_else(|| dest.to_path_buf())
    };
    Ok(Extracted { files, at })
}

fn extract_tar<R: Read>(
    reader: R,
    wanted: &dyn Fn(&str) -> bool,
    strip: &str,
    dest: &Path,
    files: &mut usize,
    found: &mut bool,
    step: &mut dyn FnMut(&str, u64) -> bool,
) -> io::Result<()> {
    let mut ar = tar::Archive::new(reader);
    for entry in ar.entries()? {
        let mut entry = entry?;
        let raw = entry.path()?.to_string_lossy().into_owned();
        let Some(name) = clean(&raw) else {
            continue;
        };
        if !wanted(&name) {
            continue;
        }
        let t = entry.header().entry_type();
        if !(t.is_file() || t.is_dir() || t.is_contiguous()) {
            continue;
        }
        *found = true;
        let rel = name.strip_prefix(strip).unwrap_or(&name);
        let Some(out) = under(dest, rel) else {
            continue;
        };
        if t.is_dir() {
            std::fs::create_dir_all(long_path(&out))?;
        } else {
            if !step(&name, entry.header().size().unwrap_or(0)) {
                return Err(cancelled());
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(long_path(parent))?;
            }
            let mut w = File::create(long_path(&out))?;
            io::copy(&mut entry, &mut w)?;
            w.flush()?;
            *files += 1;
        }
    }
    Ok(())
}

/// Reads and throws away the rest of one entry's data. In a solid 7z block the
/// next file's bytes begin where this one's end, so even a file that is not
/// wanted has to be decoded before the one after it can be read. Where files are
/// packed one to a block there is nothing to line up, so the data is left alone.
/// A failure is parked in `stop` so the caller can stop with the real reason.
fn drain(reader: &mut dyn Read, solid: bool, stop: &mut Option<io::Error>) -> bool {
    if !solid {
        return true;
    }
    match io::copy(reader, &mut io::sink()) {
        Ok(_) => true,
        Err(e) => {
            *stop = Some(e);
            false
        }
    }
}

fn extract_7z(
    archive: &Path,
    wanted: &dyn Fn(&str) -> bool,
    strip: &str,
    dest: &Path,
    files: &mut usize,
    found: &mut bool,
    step: &mut dyn FnMut(&str, u64) -> bool,
) -> io::Result<()> {
    let mut sz =
        sevenz_rust::SevenZReader::open(long_path(archive), sevenz_rust::Password::empty())
            .map_err(invalid)?;
    // A block holding more than one file is solid, and its files can only be
    // decoded in order.
    let solid = sz
        .archive()
        .folders
        .iter()
        .any(|f| f.num_unpack_sub_streams > 1);
    // The library's closure returns its own error type, so an `io::Error` is kept
    // here and answering `false` stops the walk.
    let mut stop: Option<io::Error> = None;
    let res = sz.for_each_entries(|entry, reader| {
        let Some(name) = clean(entry.name()) else {
            return Ok(drain(reader, solid, &mut stop));
        };
        if !wanted(&name) {
            return Ok(drain(reader, solid, &mut stop));
        }
        *found = true;
        let rel = name.strip_prefix(strip).unwrap_or(&name);
        let Some(out) = under(dest, rel) else {
            return Ok(drain(reader, solid, &mut stop));
        };
        if entry.is_directory() {
            if let Err(e) = std::fs::create_dir_all(long_path(&out)) {
                stop = Some(e);
                return Ok(false);
            }
            return Ok(true);
        }
        if !step(&name, entry.size()) {
            stop = Some(cancelled());
            return Ok(false);
        }
        let write = (|| -> io::Result<()> {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(long_path(parent))?;
            }
            let mut w = File::create(long_path(&out))?;
            io::copy(reader, &mut w)?;
            w.flush()?;
            Ok(())
        })();
        match write {
            Ok(()) => {
                *files += 1;
                Ok(true)
            }
            Err(e) => {
                stop = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = stop {
        return Err(e);
    }
    res.map_err(invalid)?;
    Ok(())
}

/// Whether a RAR packs its files against each other, so one file's bytes are
/// needed to reach the next and an unwanted one cannot simply be skipped. The
/// facade does not answer this, so each family is asked in its own terms.
fn rar_is_solid(ar: &rars::Archive) -> bool {
    match ar {
        rars::Archive::Rar13(a) => a.main.is_solid(),
        rars::Archive::Rar15To40(a) => a.main.is_solid() || a.files().any(|f| f.is_solid()),
        rars::Archive::Rar50Plus(a) => {
            a.main.is_solid() || a.files().any(|f| f.compression_info & 0x40 != 0)
        }
        // A family added later is not known to be solid; skipping a file it
        // needed would be an error, not a crash.
        _ => false,
    }
}

fn extract_rar(
    archive: &Path,
    wanted: &dyn Fn(&str) -> bool,
    strip: &str,
    dest: &Path,
    files: &mut usize,
    found: &mut bool,
    step: &mut dyn FnMut(&str, u64) -> bool,
) -> io::Result<()> {
    let ar = rars::ArchiveReader::read_reader(File::open(long_path(archive))?).map_err(invalid)?;
    // In a solid archive an unwanted file still has to be decoded, because the
    // next file's bytes begin where its end. Its output goes to nothing.
    let solid = rar_is_solid(&ar);
    let drain = |m: &rars::ArchiveMember| -> rars::ExtractionDecision {
        if solid && !m.meta.is_directory && !m.meta.is_redirection {
            rars::ExtractionDecision::Extract(Box::new(io::sink()))
        } else {
            rars::ExtractionDecision::Skip
        }
    };
    // The library's closure returns its own error type, so an `io::Error` is
    // kept here and answering `Stop` ends the walk with it.
    let mut stop: Option<io::Error> = None;
    let outcome = ar.extract_with_control(rars::ArchiveReadOptions::new(), |m| {
        let name = m.meta.name_lossy();
        let take = clean(&name).filter(|n| wanted(n));
        let Some(name) = take else {
            return Ok(drain(m));
        };
        *found = true;
        let rel = name.strip_prefix(strip).unwrap_or(&name);
        let Some(out) = under(dest, rel) else {
            return Ok(drain(m));
        };
        if m.meta.is_directory {
            if let Err(e) = std::fs::create_dir_all(long_path(&out)) {
                stop = Some(e);
                return Ok(rars::ExtractionDecision::Stop);
            }
            return Ok(rars::ExtractionDecision::Skip);
        }
        if !step(&name, m.meta.unpacked_size) {
            stop = Some(cancelled());
            return Ok(rars::ExtractionDecision::Stop);
        }
        if let Some(parent) = out.parent()
            && let Err(e) = std::fs::create_dir_all(long_path(parent))
        {
            stop = Some(e);
            return Ok(rars::ExtractionDecision::Stop);
        }
        match File::create(long_path(&out)) {
            Ok(w) => {
                *files += 1;
                Ok(rars::ExtractionDecision::Extract(Box::new(w)))
            }
            Err(e) => {
                stop = Some(e);
                Ok(rars::ExtractionDecision::Stop)
            }
        }
    });
    if let Some(e) = stop {
        return Err(e);
    }
    outcome.map_err(invalid)?;
    Ok(())
}

/// Where files brought out of archives are kept until the window closes.
pub fn cache_root() -> PathBuf {
    std::env::temp_dir().join(format!("rhumb-archive-{}", std::process::id()))
}

/// Makes a path inside an archive into a real one, by extracting it to the cache, and
/// returns where it is. Done once: asking again for the same thing from an archive that
/// has not changed finds it there.
pub fn materialize(path: &Path) -> io::Result<PathBuf> {
    let Some(Inside { archive, inner }) = split(path) else {
        return Ok(path.to_path_buf());
    };
    if inner.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an archive is not one file; extract it instead",
        ));
    }
    let meta = std::fs::metadata(long_path(&archive))?;
    let stamp = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        archive.hash(&mut h);
        meta.len().hash(&mut h);
        meta.modified().ok().hash(&mut h);
        h.finish()
    };
    let dest = cache_root().join(format!("{stamp:016x}"));
    let name = inner.rsplit('/').next().unwrap_or(&inner);
    let wanted = under(&dest, name).ok_or_else(|| invalid("unsafe name"))?;
    if wanted.exists() {
        return Ok(wanted);
    }
    let out = extract_to(&archive, &inner, &dest)?;
    Ok(out.at)
}

/// Removes what this window brought out of archives.
pub fn clear_own_cache() {
    let _ = std::fs::remove_dir_all(cache_root());
}

/// Removes the cache of windows that are long gone.
pub fn sweep_cache() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let mine = format!("rhumb-archive-{}", std::process::id());
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("rhumb-archive-") && name != mine {
            // Old ones only: another window may be using its own right now.
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|d| d > Duration::from_secs(24 * 3600));
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("rhumb-archive-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A zip with the given files; a name ending in `/` is a folder entry.
    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files {
            if name.ends_with('/') {
                z.add_directory(*name, opt).unwrap();
            } else {
                z.start_file(*name, opt).unwrap();
                z.write_all(body).unwrap();
            }
        }
        z.finish().unwrap();
    }

    /// A 7z with the given files; a name ending in `/` is a folder entry.
    fn make_7z(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = sevenz_rust::SevenZWriter::create(path).unwrap();
        for (name, body) in files {
            let mut e = sevenz_rust::SevenZArchiveEntry::new();
            e.name = name.to_string();
            e.has_last_modified_date = true;
            e.last_modified_date =
                sevenz_rust::nt_time::FileTime::from_unix_time(1_700_000_000).unwrap();
            if name.ends_with('/') {
                e.is_directory = true;
                w.push_archive_entry::<&[u8]>(e, None).unwrap();
            } else {
                w.push_archive_entry(e, Some(*body)).unwrap();
            }
        }
        w.finish().unwrap();
    }

    /// A RAR with the given files; a name ending in `/` is a folder entry.
    ///
    /// Built with the crate's writer, which only the test build compiles in;
    /// the program itself ships the reader alone.
    fn make_rar(path: &Path, files: &[(&str, &[u8])]) {
        let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50).store(true);
        for (name, body) in files {
            if name.ends_with('/') {
                b.add_directory(name.as_bytes().to_vec(), Some(1_700_000_000), None)
                    .unwrap();
            } else {
                b.add_bytes(
                    name.as_bytes().to_vec(),
                    body.to_vec(),
                    Some(1_700_000_000),
                    None,
                )
                .unwrap();
            }
        }
        std::fs::write(path, b.to_bytes().unwrap()).unwrap();
    }

    fn make_tar(path: &Path, files: &[(&str, &[u8])], gz: bool) {
        let f = File::create(path).unwrap();
        let w: Box<dyn Write> = if gz {
            Box::new(flate2::write::GzEncoder::new(
                f,
                flate2::Compression::fast(),
            ))
        } else {
            Box::new(f)
        };
        let mut b = tar::Builder::new(w);
        for (name, body) in files {
            let mut h = tar::Header::new_gnu();
            if name.ends_with('/') {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_size(0);
            } else {
                h.set_size(body.len() as u64);
            }
            h.set_mode(0o644);
            h.set_mtime(1_700_000_000);
            h.set_cksum();
            b.append_data(&mut h, name, *body).unwrap();
        }
        b.into_inner().unwrap().flush().unwrap();
    }

    const FILES: &[(&str, &[u8])] = &[
        ("readme.txt", b"hello"),
        ("src/main.rs", b"fn main() {}"),
        ("src/lib.rs", b"//"),
        ("src/deep/er/note.md", b"# note"),
        ("docs/", b""),
        ("docs/guide.md", b"guide"),
    ];

    fn each_format(name: &str, f: impl Fn(&Path, &str)) {
        let d = dir(name);
        for (ext, kind) in [
            ("zip", 0),
            ("tar", 1),
            ("tar.gz", 2),
            ("tgz", 2),
            ("7z", 3),
            ("rar", 4),
        ] {
            let p = d.join(format!("a.{ext}"));
            match kind {
                0 => make_zip(&p, FILES),
                1 => make_tar(&p, FILES, false),
                2 => make_tar(&p, FILES, true),
                3 => make_7z(&p, FILES),
                _ => make_rar(&p, FILES),
            }
            f(&p, ext);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    fn names(v: &[Entry]) -> Vec<String> {
        v.iter().map(|e| e.name.clone()).collect()
    }

    // ---- recognising archives ---------------------------------------------------------

    #[test]
    fn a_name_is_an_archive_by_its_extension_in_any_case() {
        for (n, k) in [
            ("a.zip", Some(Kind::Zip)),
            ("A.ZIP", Some(Kind::Zip)),
            ("lib.jar", Some(Kind::Zip)),
            ("a.tar", Some(Kind::Tar)),
            ("a.tar.gz", Some(Kind::TarGz)),
            ("a.TGZ", Some(Kind::TarGz)),
            ("a.7z", Some(Kind::SevenZ)),
            ("A.7Z", Some(Kind::SevenZ)),
            ("a.rar", Some(Kind::Rar)),
            ("A.RAR", Some(Kind::Rar)),
            ("a.gz", None),
            ("zip", None),
            ("a.zip.txt", None),
            ("a.txt", None),
            ("", None),
        ] {
            assert_eq!(kind_of(Path::new(n)), k, "{n:?}");
        }
    }

    #[test]
    fn a_folder_called_something_dot_zip_is_not_an_archive() {
        let d = dir("folder-zip");
        std::fs::create_dir_all(d.join("odd.zip/inside")).unwrap();
        assert!(!is_archive_file(&d.join("odd.zip")));
        assert!(split(&d.join("odd.zip/inside")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_path_is_split_at_the_archive_file() {
        let d = dir("split");
        make_zip(&d.join("p.zip"), FILES);
        let s = split(&d.join("p.zip")).unwrap();
        assert_eq!(s.archive, d.join("p.zip"));
        assert_eq!(s.inner, "");
        let s = split(&d.join("p.zip").join("src").join("main.rs")).unwrap();
        assert_eq!(s.archive, d.join("p.zip"));
        assert_eq!(s.inner, "src/main.rs");
        assert!(is_virtual(&d.join("p.zip/src")));
        assert!(!is_virtual(&d));
        assert!(!is_virtual(&d.join("missing.zip")));
        assert!(!is_virtual(&d.join("missing.zip/x")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn join_and_split_agree() {
        let d = dir("roundtrip");
        make_zip(&d.join("p.zip"), FILES);
        for inner in ["", "src", "src/deep/er", "src/deep/er/note.md"] {
            let p = join(&d.join("p.zip"), inner);
            let s = split(&p).unwrap();
            assert_eq!(s.inner, inner);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- listing -------------------------------------------------------------------------

    #[test]
    fn the_top_of_an_archive_lists_its_files_and_folders_in_every_format() {
        each_format("top", |p, ext| {
            let top = list(p, "").unwrap();
            let mut n = names(&top);
            n.sort();
            assert_eq!(n, vec!["docs", "readme.txt", "src"], "{ext}");
            let src = top.iter().find(|e| e.name == "src").unwrap();
            assert!(src.is_dir);
            assert_eq!(src.path, join(p, "src"));
            let readme = top.iter().find(|e| e.name == "readme.txt").unwrap();
            assert!(!readme.is_dir);
            assert_eq!(readme.size, 5);
        });
    }

    #[test]
    fn a_folder_that_has_no_entry_of_its_own_is_made_up_from_what_is_in_it() {
        each_format("implied", |p, ext| {
            // `src/deep/er/` and `src/deep/` have no entries, only a file below them.
            let mut n = names(&list(p, "src").unwrap());
            n.sort();
            assert_eq!(n, vec!["deep", "lib.rs", "main.rs"], "{ext}");
            assert_eq!(names(&list(p, "src/deep").unwrap()), vec!["er"]);
            assert_eq!(names(&list(p, "src/deep/er").unwrap()), vec!["note.md"]);
        });
    }

    #[test]
    fn a_listing_of_something_that_is_not_there_is_an_error() {
        each_format("missing", |p, ext| {
            let e = list(p, "nope").unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::NotFound, "{ext}");
            assert!(list(p, "readme.txt/x").is_err(), "{ext}");
        });
    }

    #[test]
    fn an_empty_folder_entry_lists_as_empty() {
        let d = dir("emptydir2");
        make_zip(&d.join("e.zip"), &[("only/", b"")]);
        assert_eq!(names(&list(&d.join("e.zip"), "").unwrap()), vec!["only"]);
        assert!(list(&d.join("e.zip"), "only").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_archive_lists_as_empty() {
        let d = dir("emptyarchive");
        make_zip(&d.join("e.zip"), &[]);
        assert!(list(&d.join("e.zip"), "").unwrap().is_empty());
        make_tar(&d.join("e.tar"), &[], false);
        assert!(list(&d.join("e.tar"), "").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sizes_and_dates_come_from_the_archive() {
        let d = dir("meta");
        make_tar(&d.join("m.tar"), FILES, false);
        let top = list(&d.join("m.tar"), "").unwrap();
        let readme = top.iter().find(|e| e.name == "readme.txt").unwrap();
        assert_eq!(readme.size, 5);
        assert_eq!(
            readme.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        make_zip(&d.join("m.zip"), FILES);
        let top = list(&d.join("m.zip"), "").unwrap();
        let readme = top.iter().find(|e| e.name == "readme.txt").unwrap();
        assert_eq!(readme.size, 5);
        assert!(readme.modified.is_some(), "a zip stores a date too");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn backslashes_in_names_and_odd_slashes_are_made_ordinary() {
        let d = dir("slashes");
        make_zip(
            &d.join("w.zip"),
            &[
                ("win\\dir\\file.txt", b"1"),
                ("./dot/./x.txt", b"2"),
                ("a//b.txt", b"3"),
            ],
        );
        let mut n = names(&list(&d.join("w.zip"), "").unwrap());
        n.sort();
        assert_eq!(n, vec!["a", "dot", "win"]);
        assert_eq!(
            names(&list(&d.join("w.zip"), "win/dir").unwrap()),
            vec!["file.txt"]
        );
        assert_eq!(names(&list(&d.join("w.zip"), "a").unwrap()), vec!["b.txt"]);
        // And the inner path given with the other slash, or with extra ones, finds them.
        assert_eq!(
            names(&list(&d.join("w.zip"), "win\\dir").unwrap()),
            vec!["file.txt"]
        );
        assert_eq!(
            names(&list(&d.join("w.zip"), "/win//dir/").unwrap()),
            vec!["file.txt"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_that_climb_out_or_start_from_a_root_are_not_listed() {
        let d = dir("unsafe-list");
        make_zip(
            &d.join("u.zip"),
            &[
                ("../evil.txt", b"x"),
                ("a/../../evil2.txt", b"x"),
                ("C:/win.txt", b"x"),
                ("ok.txt", b"x"),
            ],
        );
        assert_eq!(names(&list(&d.join("u.zip"), "").unwrap()), vec!["ok.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unicode_and_awkward_names_survive() {
        let d = dir("unicode");
        make_zip(
            &d.join("n.zip"),
            &[
                ("日本語/ファイル.txt", b"x"),
                ("with space (1).txt", b"y"),
                (".hidden", b"z"),
                ("café ☕.md", b"w"),
            ],
        );
        let top = list(&d.join("n.zip"), "").unwrap();
        let mut n = names(&top);
        n.sort();
        assert_eq!(
            n,
            vec![".hidden", "café ☕.md", "with space (1).txt", "日本語"]
        );
        assert!(top.iter().find(|e| e.name == ".hidden").unwrap().hidden);
        assert_eq!(
            names(&list(&d.join("n.zip"), "日本語").unwrap()),
            vec!["ファイル.txt"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_that_is_both_a_file_and_a_folder_shows_as_a_folder_once() {
        let d = dir("both");
        make_zip(&d.join("b.zip"), &[("x", b"file"), ("x/inner.txt", b"y")]);
        let top = list(&d.join("b.zip"), "").unwrap();
        assert_eq!(top.len(), 1);
        assert!(top[0].is_dir);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_same_name_twice_is_listed_once() {
        let d = dir("dup");
        make_zip(&d.join("d.zip"), &[("a.txt", b"one"), ("b/a.txt", b"x")]);
        assert_eq!(
            names(&list(&d.join("d.zip"), "").unwrap()),
            vec!["a.txt", "b"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_very_deep_archive_lists_level_by_level() {
        let d = dir("deep");
        let deep: String = (0..60).map(|i| format!("d{i}/")).collect::<String>() + "leaf.txt";
        make_zip(&d.join("deep.zip"), &[(deep.as_str(), b"x")]);
        let mut inner = String::new();
        for i in 0..60 {
            assert_eq!(
                names(&list(&d.join("deep.zip"), &inner).unwrap()),
                vec![format!("d{i}")]
            );
            inner = if inner.is_empty() {
                format!("d{i}")
            } else {
                format!("{inner}/d{i}")
            };
        }
        assert_eq!(
            names(&list(&d.join("deep.zip"), &inner).unwrap()),
            vec!["leaf.txt"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_big_archive_lists_quickly_and_the_second_listing_uses_the_cache() {
        let d = dir("big");
        let names_owned: Vec<String> = (0..4000)
            .map(|i| format!("dir{}/file{i}.txt", i % 40))
            .collect();
        let files: Vec<(&str, &[u8])> = names_owned
            .iter()
            .map(|n| (n.as_str(), &b"x"[..]))
            .collect();
        make_zip(&d.join("big.zip"), &files);
        let t = std::time::Instant::now();
        let top = list(&d.join("big.zip"), "").unwrap();
        let first = t.elapsed();
        assert_eq!(top.len(), 40);
        let t = std::time::Instant::now();
        for i in 0..40 {
            assert_eq!(
                list(&d.join("big.zip"), &format!("dir{i}")).unwrap().len(),
                100
            );
        }
        let rest = t.elapsed();
        assert!(first < Duration::from_millis(2000), "{first:?}");
        assert!(
            rest < Duration::from_millis(500),
            "forty listings took {rest:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_changed_archive_is_read_again() {
        let d = dir("changed");
        let p = d.join("c.zip");
        make_zip(&p, &[("one.txt", b"1")]);
        assert_eq!(names(&list(&p, "").unwrap()), vec!["one.txt"]);
        // A different size, so the stamp differs even where the clock is coarse.
        make_zip(&p, &[("two.txt", b"22"), ("three.txt", b"333")]);
        let mut n = names(&list(&p, "").unwrap());
        n.sort();
        assert_eq!(n, vec!["three.txt", "two.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_is_not_an_archive_is_an_error_and_not_a_panic() {
        let d = dir("corrupt");
        std::fs::write(d.join("bad.zip"), b"this is not a zip file at all").unwrap();
        assert!(list(&d.join("bad.zip"), "").is_err());
        std::fs::write(d.join("bad.tar"), vec![7u8; 700]).unwrap();
        let _ = list(&d.join("bad.tar"), "");
        std::fs::write(d.join("bad.tgz"), b"not gzip").unwrap();
        assert!(list(&d.join("bad.tgz"), "").is_err());
        std::fs::write(d.join("empty.zip"), b"").unwrap();
        assert!(list(&d.join("empty.zip"), "").is_err());
        assert!(list(&d.join("absent.zip"), "").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_truncated_archive_does_not_panic() {
        let d = dir("truncated");
        make_zip(&d.join("t.zip"), FILES);
        let bytes = std::fs::read(d.join("t.zip")).unwrap();
        std::fs::write(d.join("cut.zip"), &bytes[..bytes.len() / 2]).unwrap();
        assert!(list(&d.join("cut.zip"), "").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn whether_a_place_inside_is_a_folder_is_answered() {
        each_format("isdir", |p, ext| {
            assert!(is_dir_inside(p, ""), "{ext}");
            assert!(is_dir_inside(p, "src"), "{ext}");
            assert!(is_dir_inside(p, "src/deep"), "{ext}");
            assert!(is_dir_inside(p, "docs"), "{ext}");
            assert!(!is_dir_inside(p, "readme.txt"), "{ext}");
            assert!(!is_dir_inside(p, "src/main.rs"), "{ext}");
            assert!(!is_dir_inside(p, "nope"), "{ext}");
        });
    }

    // ---- extracting ----------------------------------------------------------------------

    #[test]
    fn one_file_is_extracted_under_its_own_name() {
        each_format("extract-file", |p, ext| {
            let out = dir(&format!("out-file-{ext}"));
            let r = extract_to(p, "src/main.rs", &out).unwrap();
            assert_eq!(r.files, 1, "{ext}");
            assert_eq!(r.at, out.join("main.rs"));
            assert_eq!(std::fs::read(out.join("main.rs")).unwrap(), b"fn main() {}");
            let _ = std::fs::remove_dir_all(&out);
        });
    }

    #[test]
    fn a_folder_is_extracted_with_everything_in_it() {
        each_format("extract-dir", |p, ext| {
            let out = dir(&format!("out-dir-{ext}"));
            let r = extract_to(p, "src", &out).unwrap();
            assert_eq!(r.files, 3, "{ext}");
            assert_eq!(r.at, out.join("src"));
            assert_eq!(std::fs::read(out.join("src/lib.rs")).unwrap(), b"//");
            assert_eq!(
                std::fs::read(out.join("src/deep/er/note.md")).unwrap(),
                b"# note"
            );
            assert!(!out.join("readme.txt").exists(), "and nothing else");
            let _ = std::fs::remove_dir_all(&out);
        });
    }

    #[test]
    fn a_folder_inside_a_folder_lands_under_its_own_name_only() {
        each_format("extract-nested", |p, ext| {
            let out = dir(&format!("out-nest-{ext}"));
            extract_to(p, "src/deep", &out).unwrap();
            assert!(out.join("deep/er/note.md").is_file(), "{ext}");
            assert!(!out.join("src").exists(), "{ext}");
            let _ = std::fs::remove_dir_all(&out);
        });
    }

    #[test]
    fn the_whole_archive_is_extracted_as_its_contents() {
        each_format("extract-all", |p, ext| {
            let out = dir(&format!("out-all-{ext}"));
            let r = extract_to(p, "", &out).unwrap();
            assert_eq!(r.files, 5, "{ext}");
            assert_eq!(r.at, out);
            assert!(out.join("readme.txt").is_file());
            assert!(out.join("src/main.rs").is_file());
            assert!(out.join("docs/guide.md").is_file());
            let _ = std::fs::remove_dir_all(&out);
        });
    }

    #[test]
    fn extracting_something_that_is_not_there_is_an_error() {
        each_format("extract-missing", |p, ext| {
            let out = dir(&format!("out-missing-{ext}"));
            assert!(extract_to(p, "nothing/here", &out).is_err(), "{ext}");
            let _ = std::fs::remove_dir_all(&out);
        });
    }

    #[test]
    fn a_name_that_climbs_out_of_the_folder_is_never_written() {
        let d = dir("zipslip");
        make_zip(
            &d.join("slip.zip"),
            &[
                ("../escaped.txt", b"bad"),
                ("sub/../../escaped2.txt", b"bad"),
                ("fine.txt", b"ok"),
            ],
        );
        let out = d.join("out");
        let r = extract_to(&d.join("slip.zip"), "", &out).unwrap();
        assert_eq!(r.files, 1);
        assert!(out.join("fine.txt").is_file());
        assert!(!d.join("escaped.txt").exists());
        assert!(!d.join("escaped2.txt").exists());
        assert!(!out.parent().unwrap().join("escaped.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_with_a_drive_in_it_is_never_written_outside() {
        let d = dir("drive-name");
        make_zip(
            &d.join("drv.zip"),
            &[("C:/Windows/evil.txt", b"x"), ("/abs/evil.txt", b"x")],
        );
        let out = d.join("out");
        extract_to(&d.join("drv.zip"), "", &out).unwrap();
        assert!(!Path::new("C:/Windows/evil.txt").exists());
        // The absolute name is made relative, so it lands inside.
        assert!(out.join("abs/evil.txt").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn extracting_over_an_existing_file_replaces_it() {
        let d = dir("overwrite");
        make_zip(&d.join("o.zip"), &[("f.txt", b"new")]);
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("f.txt"), b"old old old").unwrap();
        extract_to(&d.join("o.zip"), "f.txt", &out).unwrap();
        assert_eq!(std::fs::read(out.join("f.txt")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_big_file_is_extracted_whole() {
        let d = dir("bigfile");
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        make_zip(&d.join("b.zip"), &[("big.bin", &big)]);
        let out = d.join("out");
        extract_to(&d.join("b.zip"), "big.bin", &out).unwrap();
        assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), big);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_files_and_empty_folders_are_extracted() {
        let d = dir("emptyfiles");
        make_zip(&d.join("e.zip"), &[("zero.txt", b""), ("hollow/", b"")]);
        let out = d.join("out");
        let r = extract_to(&d.join("e.zip"), "", &out).unwrap();
        assert_eq!(r.files, 1);
        assert_eq!(std::fs::metadata(out.join("zero.txt")).unwrap().len(), 0);
        assert!(out.join("hollow").is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- bringing a file out to open it -----------------------------------------------------

    #[test]
    fn a_path_inside_an_archive_becomes_a_real_file() {
        let d = dir("materialize");
        make_zip(&d.join("m.zip"), FILES);
        let virt = d.join("m.zip").join("src").join("main.rs");
        let real = materialize(&virt).unwrap();
        assert!(real.is_file());
        assert_eq!(real.file_name().unwrap(), "main.rs");
        assert_eq!(std::fs::read(&real).unwrap(), b"fn main() {}");
        assert!(real.starts_with(cache_root()));
        let _ = std::fs::remove_dir_all(&d);
        // The cache is process-wide; wiping it here raced other tests reading it.
        // It is left for the app's own sweep, which clears stale ones on launch.
    }

    #[test]
    fn asking_twice_for_the_same_file_gives_the_same_one_without_extracting_again() {
        let d = dir("materialize-twice");
        make_zip(&d.join("m.zip"), FILES);
        let virt = d.join("m.zip").join("readme.txt");
        let a = materialize(&virt).unwrap();
        // Changed by hand: if it were extracted again this would be undone.
        std::fs::write(&a, b"edited").unwrap();
        let b = materialize(&virt).unwrap();
        assert_eq!(a, b);
        assert_eq!(std::fs::read(&b).unwrap(), b"edited");
        let _ = std::fs::remove_dir_all(&d);
        // The cache is process-wide; wiping it here raced other tests reading it.
        // It is left for the app's own sweep, which clears stale ones on launch.
    }

    #[test]
    fn a_folder_inside_an_archive_can_be_made_real_too() {
        let d = dir("materialize-dir");
        make_zip(&d.join("m.zip"), FILES);
        let real = materialize(&d.join("m.zip").join("src")).unwrap();
        assert!(real.is_dir());
        assert!(real.join("main.rs").is_file());
        let _ = std::fs::remove_dir_all(&d);
        // The cache is process-wide; wiping it here raced other tests reading it.
        // It is left for the app's own sweep, which clears stale ones on launch.
    }

    #[test]
    fn an_ordinary_path_is_left_alone() {
        let d = dir("materialize-plain");
        std::fs::write(d.join("x.txt"), b"x").unwrap();
        assert_eq!(materialize(&d.join("x.txt")).unwrap(), d.join("x.txt"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_top_of_an_archive_is_not_one_file() {
        let d = dir("materialize-top");
        make_zip(&d.join("m.zip"), FILES);
        assert!(materialize(&d.join("m.zip")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_changed_archive_gets_its_own_cache_so_old_contents_are_not_served() {
        let d = dir("materialize-changed");
        let p = d.join("m.zip");
        make_zip(&p, &[("f.txt", b"first")]);
        let a = materialize(&p.join("f.txt")).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        make_zip(&p, &[("f.txt", b"second one")]);
        let b = materialize(&p.join("f.txt")).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"second one");
        let _ = std::fs::remove_dir_all(&d);
        // The cache is process-wide; wiping it here raced other tests reading it.
        // It is left for the app's own sweep, which clears stale ones on launch.
    }

    #[test]
    fn counting_what_an_extraction_would_write() {
        each_format("count", |p, ext| {
            assert_eq!(
                count_files(p, "").unwrap(),
                (5, 5 + 12 + 2 + 6 + 5),
                "{ext}"
            );
            assert_eq!(count_files(p, "src").unwrap(), (3, 12 + 2 + 6), "{ext}");
            assert_eq!(count_files(p, "readme.txt").unwrap(), (1, 5), "{ext}");
            assert_eq!(count_files(p, "nope").unwrap(), (0, 0), "{ext}");
        });
    }

    #[test]
    fn an_extraction_reports_each_file_and_can_be_stopped() {
        each_format("step", |p, ext| {
            let out = dir(&format!("out-step-{ext}"));
            let mut seen = Vec::new();
            extract_with(p, "", &out, &mut |n, _| {
                seen.push(n.to_owned());
                true
            })
            .unwrap();
            seen.sort();
            assert_eq!(
                seen,
                vec![
                    "docs/guide.md",
                    "readme.txt",
                    "src/deep/er/note.md",
                    "src/lib.rs",
                    "src/main.rs"
                ],
                "{ext}"
            );
            let out2 = dir(&format!("out-step2-{ext}"));
            let mut n = 0;
            let e = extract_with(p, "", &out2, &mut |_, _| {
                n += 1;
                n < 3
            })
            .unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::Interrupted, "{ext}");
            let _ = std::fs::remove_dir_all(&out);
            let _ = std::fs::remove_dir_all(&out2);
        });
    }

    #[test]
    fn the_zip_date_is_converted_to_the_right_day() {
        // 2024-02-29 12:34:56, a leap day.
        let dt = zip::DateTime::from_date_and_time(2024, 2, 29, 12, 34, 56).unwrap();
        let t = zip_time(dt).unwrap();
        let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 1_709_210_096);
        let epoch = zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(
            zip_time(epoch)
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            315_532_800
        );
    }

    // ---- 7z ------------------------------------------------------------------------------

    #[test]
    fn a_7z_is_recognised_split_and_listed() {
        let d = dir("sevenz");
        let p = d.join("a.7z");
        make_7z(&p, FILES);
        assert!(is_archive_file(&p));
        let s = split(&p.join("src").join("main.rs")).unwrap();
        assert_eq!(s.archive, p);
        assert_eq!(s.inner, "src/main.rs");
        assert!(is_virtual(&p.join("src")));
        let mut n = names(&list(&p, "").unwrap());
        n.sort();
        assert_eq!(n, vec!["docs", "readme.txt", "src"]);
        let readme = list(&p, "")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "readme.txt")
            .unwrap();
        assert!(!readme.is_dir);
        assert_eq!(readme.size, 5);
        assert_eq!(
            readme.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "the archive records a date"
        );
        assert!(is_dir_inside(&p, "src/deep"));
        assert!(!is_dir_inside(&p, "readme.txt"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn one_file_is_extracted_from_a_7z() {
        let d = dir("sevenz-extract");
        let p = d.join("a.7z");
        make_7z(&p, FILES);
        let out = d.join("out");
        let r = extract_to(&p, "src/main.rs", &out).unwrap();
        assert_eq!(r.files, 1);
        assert_eq!(r.at, out.join("main.rs"));
        assert_eq!(std::fs::read(out.join("main.rs")).unwrap(), b"fn main() {}");
        assert!(!out.join("readme.txt").exists(), "and nothing else");
        // And the same file, asked for by its virtual path, becomes real.
        let real = materialize(&p.join("src").join("main.rs")).unwrap();
        assert_eq!(std::fs::read(&real).unwrap(), b"fn main() {}");
        let _ = std::fs::remove_dir_all(&d);
        // The cache is process-wide; wiping it here raced other tests reading it.
        // It is left for the app's own sweep, which clears stale ones on launch.
    }

    #[test]
    fn a_solid_7z_still_extracts_one_file_by_decoding_the_ones_before_it() {
        let d = dir("sevenz-solid");
        let src = d.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("first.txt"), b"first").unwrap();
        std::fs::write(src.join("second.txt"), b"second").unwrap();
        std::fs::write(src.join("third.txt"), b"third").unwrap();
        let p = d.join("solid.7z");
        let mut w = sevenz_rust::SevenZWriter::create(&p).unwrap();
        // All the files go into one block, so the wanted one cannot be reached
        // without decoding what comes before it.
        w.push_source_path(&src, |_| true).unwrap();
        w.finish().unwrap();
        let mut n = names(&list(&p, "").unwrap());
        n.sort();
        assert_eq!(n, vec!["first.txt", "second.txt", "third.txt"]);
        let out = d.join("out");
        let r = extract_to(&p, "third.txt", &out).unwrap();
        assert_eq!(r.files, 1);
        assert_eq!(std::fs::read(out.join("third.txt")).unwrap(), b"third");
        assert!(!out.join("first.txt").exists(), "and nothing else");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_corrupt_7z_is_an_error_and_not_a_panic() {
        let d = dir("bad-sevenz");
        std::fs::write(d.join("bad.7z"), b"this is not a 7z file at all").unwrap();
        assert!(list(&d.join("bad.7z"), "").is_err());
        std::fs::write(d.join("empty.7z"), b"").unwrap();
        assert!(list(&d.join("empty.7z"), "").is_err());
        // The right signature but nothing behind it.
        std::fs::write(d.join("cut.7z"), b"7z\xbc\xaf\x27\x1c\x00\x02short").unwrap();
        assert!(list(&d.join("cut.7z"), "").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- rar -----------------------------------------------------------------------------

    #[test]
    fn a_rar_is_recognised_split_listed_and_extracted() {
        let d = dir("rar");
        let p = d.join("a.rar");
        make_rar(&p, FILES);
        assert!(is_archive_file(&p));
        let s = split(&p.join("src").join("main.rs")).unwrap();
        assert_eq!(s.archive, p);
        assert_eq!(s.inner, "src/main.rs");
        assert!(is_virtual(&p.join("src")));
        let mut n = names(&list(&p, "").unwrap());
        n.sort();
        assert_eq!(n, vec!["docs", "readme.txt", "src"]);
        let readme = list(&p, "")
            .unwrap()
            .into_iter()
            .find(|e| e.name == "readme.txt")
            .unwrap();
        assert!(!readme.is_dir);
        assert_eq!(readme.size, 5);
        assert_eq!(
            readme.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "the archive records a date"
        );
        assert!(is_dir_inside(&p, "src/deep"));
        assert!(!is_dir_inside(&p, "readme.txt"));
        // A file comes out of it, and the same one by its virtual path.
        let out = d.join("out");
        let r = extract_to(&p, "src/main.rs", &out).unwrap();
        assert_eq!(r.files, 1);
        assert_eq!(r.at, out.join("main.rs"));
        assert_eq!(std::fs::read(out.join("main.rs")).unwrap(), b"fn main() {}");
        assert!(!out.join("readme.txt").exists(), "and nothing else");
        let real = materialize(&p.join("src").join("main.rs")).unwrap();
        assert_eq!(std::fs::read(&real).unwrap(), b"fn main() {}");
        let _ = std::fs::remove_dir_all(&d);
        // The cache is process-wide; wiping it here raced other tests reading it.
        // It is left for the app's own sweep, which clears stale ones on launch.
    }

    #[test]
    fn a_solid_rar_still_extracts_one_file_by_decoding_the_ones_before_it() {
        let d = dir("rar-solid");
        let p = d.join("solid.rar");
        let mut b = rars::Builder::new(rars::ArchiveVersion::Rar50)
            .store(true)
            .solid(true);
        for (name, body) in [
            ("first.txt", b"first".as_slice()),
            ("second.txt", b"second".as_slice()),
            ("third.txt", b"third".as_slice()),
        ] {
            b.add_bytes(
                name.as_bytes().to_vec(),
                body.to_vec(),
                Some(1_700_000_000),
                None,
            )
            .unwrap();
        }
        std::fs::write(&p, b.to_bytes().unwrap()).unwrap();
        let mut n = names(&list(&p, "").unwrap());
        n.sort();
        assert_eq!(n, vec!["first.txt", "second.txt", "third.txt"]);
        // The wanted file is last, so its bytes can only be reached by decoding
        // what comes before it, which is what a solid archive demands.
        assert!(rar_is_solid(&rars::ArchiveReader::read_path(&p).unwrap()));
        let out = d.join("out");
        let r = extract_to(&p, "third.txt", &out).unwrap();
        assert_eq!(r.files, 1);
        assert_eq!(std::fs::read(out.join("third.txt")).unwrap(), b"third");
        assert!(!out.join("first.txt").exists(), "and nothing else");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_corrupt_rar_is_an_error_and_not_a_panic() {
        let d = dir("bad-rar");
        std::fs::write(d.join("bad.rar"), b"this is not a rar file at all").unwrap();
        assert!(list(&d.join("bad.rar"), "").is_err());
        std::fs::write(d.join("empty.rar"), b"").unwrap();
        assert!(list(&d.join("empty.rar"), "").is_err());
        // The RAR5 signature but nothing behind it.
        std::fs::write(d.join("cut.rar"), b"Rar!\x1a\x07\x01\x00short").unwrap();
        assert!(list(&d.join("cut.rar"), "").is_err());
        // And the older signature, likewise cut off.
        std::fs::write(d.join("old.rar"), b"Rar!\x1a\x07\x00\x00short").unwrap();
        assert!(list(&d.join("old.rar"), "").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clean_names_are_tidy_and_the_unsafe_ones_are_refused() {
        assert_eq!(clean("a/b/c.txt").as_deref(), Some("a/b/c.txt"));
        assert_eq!(clean("a\\b").as_deref(), Some("a/b"));
        assert_eq!(clean("/a/b/").as_deref(), Some("a/b"));
        assert_eq!(clean("./a//b/.").as_deref(), Some("a/b"));
        assert_eq!(clean(""), None);
        assert_eq!(clean("/"), None);
        assert_eq!(clean(".."), None);
        assert_eq!(clean("a/../b"), None);
        assert_eq!(clean("C:/x"), None);
        assert_eq!(clean("c:\\x"), None);
        assert_eq!(clean("ok:name").as_deref(), Some("ok:name"));
    }

    // ---- writing into a zip --------------------------------------------------------------

    /// A real file with `body`, under `dir`, returning its path.
    fn source(dir: &Path, rel: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn a_file_and_a_folder_are_added_into_a_zip() {
        let d = dir("zip-add");
        let p = d.join("a.zip");
        make_zip(&p, &[("keep.txt", b"keep")]);
        let loose = source(&d, "loose.txt", b"loose");
        source(&d, "src/one.txt", b"one");
        source(&d, "src/nested/two.txt", b"two");

        let n = add_to_zip(&p, "", &[loose.clone(), d.join("src")]).unwrap();
        assert_eq!(n, 3, "the loose file and the folder's two files");

        // The original archive still opens, and now holds everything.
        let mut top = names(&list(&p, "").unwrap());
        top.sort();
        assert_eq!(top, vec!["keep.txt", "loose.txt", "src"]);
        let mut inner = names(&list(&p, "src").unwrap());
        inner.sort();
        assert_eq!(inner, vec!["nested", "one.txt"]);
        assert_eq!(names(&list(&p, "src/nested").unwrap()), vec!["two.txt"]);

        // And the bytes are the bytes.
        let out = d.join("out");
        extract_to(&p, "src", &out).unwrap();
        assert_eq!(std::fs::read(out.join("src/one.txt")).unwrap(), b"one");
        assert_eq!(
            std::fs::read(out.join("src/nested/two.txt")).unwrap(),
            b"two"
        );
        extract_to(&p, "loose.txt", &out).unwrap();
        assert_eq!(std::fs::read(out.join("loose.txt")).unwrap(), b"loose");
        extract_to(&p, "keep.txt", &out).unwrap();
        assert_eq!(std::fs::read(out.join("keep.txt")).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn files_add_under_a_folder_inside_the_zip() {
        let d = dir("zip-add-inner");
        let p = d.join("a.zip");
        make_zip(&p, FILES);
        let new = source(&d, "new.txt", b"new");

        let n = add_to_zip(&p, "src", &[new]).unwrap();
        assert_eq!(n, 1);
        let mut inner = names(&list(&p, "src").unwrap());
        inner.sort();
        assert_eq!(inner, vec!["deep", "lib.rs", "main.rs", "new.txt"]);
        let out = d.join("out");
        extract_to(&p, "src/new.txt", &out).unwrap();
        assert_eq!(std::fs::read(out.join("new.txt")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_folder_added_to_a_zip_survives() {
        let d = dir("zip-add-empty");
        let p = d.join("a.zip");
        make_zip(&p, &[]);
        std::fs::create_dir_all(d.join("hollow")).unwrap();

        let n = add_to_zip(&p, "", &[d.join("hollow")]).unwrap();
        assert_eq!(n, 0, "a folder entry is not a file");
        assert_eq!(names(&list(&p, "").unwrap()), vec!["hollow"]);
        assert!(list(&p, "hollow").unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_entry_and_a_folder_subtree_are_removed_from_a_zip() {
        let d = dir("zip-remove");
        let p = d.join("a.zip");
        make_zip(&p, FILES);

        let n = remove_from_zip(&p, &["readme.txt".into(), "src".into()]).unwrap();
        assert_eq!(n, 4, "readme and the three files under src");
        let top = names(&list(&p, "").unwrap());
        assert_eq!(top, vec!["docs"], "the rest survives");
        assert!(list(&p, "src").is_err(), "the folder is gone");
        assert!(list(&p, "readme.txt").is_err());
        let out = d.join("out");
        extract_to(&p, "docs/guide.md", &out).unwrap();
        assert_eq!(std::fs::read(out.join("guide.md")).unwrap(), b"guide");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_add_leaves_the_zip_as_it_was() {
        let d = dir("zip-add-fail");
        let p = d.join("a.zip");
        make_zip(&p, &[("keep.txt", b"keep")]);
        let before = std::fs::read(&p).unwrap();

        // The source is gone, so the add cannot finish.
        assert!(add_to_zip(&p, "", &[d.join("not-there.txt")]).is_err());
        assert_eq!(
            std::fs::read(&p).unwrap(),
            before,
            "the archive must not change"
        );
        assert_eq!(names(&list(&p, "").unwrap()), vec!["keep.txt"]);
        // And no half-written temporary file is left behind.
        let leftovers: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_remove_leaves_the_zip_as_it_was() {
        let d = dir("zip-remove-fail");
        // A file that looks like a zip by its name but is not one cannot be read,
        // so the rewrite fails with the bytes untouched.
        let p = d.join("a.zip");
        std::fs::write(&p, b"this is not a zip at all").unwrap();
        let before = std::fs::read(&p).unwrap();
        assert!(remove_from_zip(&p, &["anything".into()]).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), before);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_a_zip_can_be_written_into() {
        let d = dir("zip-only");
        make_tar(&d.join("a.tar"), FILES, false);
        make_7z(&d.join("a.7z"), FILES);
        make_rar(&d.join("a.rar"), FILES);
        let loose = source(&d, "x.txt", b"x");
        for name in ["a.tar", "a.7z", "a.rar"] {
            let p = d.join(name);
            assert!(
                add_to_zip(&p, "", std::slice::from_ref(&loose)).is_err(),
                "{name}"
            );
            assert!(
                remove_from_zip(&p, &["readme.txt".into()]).is_err(),
                "{name}"
            );
        }
        // A plain file is not an archive at all.
        let plain = source(&d, "plain.txt", b"x");
        assert!(add_to_zip(&plain, "", &[loose]).is_err());
        assert!(remove_from_zip(&plain, &["x".into()]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn adding_under_an_unsafe_folder_is_refused() {
        let d = dir("zip-add-unsafe");
        let p = d.join("a.zip");
        make_zip(&p, &[("keep.txt", b"keep")]);
        let before = std::fs::read(&p).unwrap();
        let loose = source(&d, "x.txt", b"x");
        assert!(add_to_zip(&p, "../out", &[loose]).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), before);
        let _ = std::fs::remove_dir_all(&d);
    }
}
