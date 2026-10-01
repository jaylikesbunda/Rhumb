//! Archives as folders.
//!
//! A `.zip`, `.tar` or `.tar.gz` can be opened like a folder: its path followed by a
//! path inside it, `C:\files\photos.zip\2024\beach.jpg`, names a place the file list
//! can show, the address bar can say and the history can come back to. Nothing on the
//! disk has such a path, so this module is the one place that knows how to read it:
//! [`split`] finds where the archive stops and the path inside starts, [`list`] reads
//! one level of what is inside, and [`extract_to`] and [`materialize`] bring files out
//! to somewhere real, which is what opening, copying and dragging need.
//!
//! Archives are read-only here: nothing is ever written back into one.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::fs_model::Entry;

/// The archive formats that can be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Zip,
    Tar,
    TarGz,
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
    let meta = std::fs::metadata(archive)?;
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
        Kind::Tar => read_tar(BufReader::new(File::open(archive)?))?,
        Kind::TarGz => read_tar(flate2::read::GzDecoder::new(BufReader::new(File::open(
            archive,
        )?)))?,
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
    let mut z = zip::ZipArchive::new(BufReader::new(File::open(archive)?)).map_err(invalid)?;
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
    std::fs::create_dir_all(dest)?;
    let kind = kind_of(archive).ok_or_else(|| invalid("not an archive"))?;
    let mut files = 0usize;
    let mut found = false;
    match kind {
        Kind::Zip => {
            let mut z =
                zip::ZipArchive::new(BufReader::new(File::open(archive)?)).map_err(invalid)?;
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
                    std::fs::create_dir_all(&out)?;
                } else {
                    if !step(&name, f.size()) {
                        return Err(cancelled());
                    }
                    if let Some(parent) = out.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut w = File::create(&out)?;
                    io::copy(&mut f, &mut w)?;
                    w.flush()?;
                    files += 1;
                }
            }
        }
        Kind::Tar => extract_tar(
            BufReader::new(File::open(archive)?),
            &wanted,
            &strip,
            dest,
            &mut files,
            &mut found,
            step,
        )?,
        Kind::TarGz => extract_tar(
            flate2::read::GzDecoder::new(BufReader::new(File::open(archive)?)),
            &wanted,
            &strip,
            dest,
            &mut files,
            &mut found,
            step,
        )?,
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
            std::fs::create_dir_all(&out)?;
        } else {
            if !step(&name, entry.header().size().unwrap_or(0)) {
                return Err(cancelled());
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut w = File::create(&out)?;
            io::copy(&mut entry, &mut w)?;
            w.flush()?;
            *files += 1;
        }
    }
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
    let meta = std::fs::metadata(&archive)?;
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
        for (ext, kind) in [("zip", 0), ("tar", 1), ("tar.gz", 2), ("tgz", 2)] {
            let p = d.join(format!("a.{ext}"));
            match kind {
                0 => make_zip(&p, FILES),
                1 => make_tar(&p, FILES, false),
                _ => make_tar(&p, FILES, true),
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
        let _ = std::fs::remove_dir_all(cache_root());
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
        let _ = std::fs::remove_dir_all(cache_root());
    }

    #[test]
    fn a_folder_inside_an_archive_can_be_made_real_too() {
        let d = dir("materialize-dir");
        make_zip(&d.join("m.zip"), FILES);
        let real = materialize(&d.join("m.zip").join("src")).unwrap();
        assert!(real.is_dir());
        assert!(real.join("main.rs").is_file());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(cache_root());
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
        let _ = std::fs::remove_dir_all(cache_root());
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
}
