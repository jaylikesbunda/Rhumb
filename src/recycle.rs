//! The Windows Recycle Bin as a place the file list can show.
//!
//! A deleted item lives as a pair of files in `$Recycle.Bin\<user SID>\`: an
//! `$I…` record that names what was deleted, when and how big it was, and an
//! `$R…` file or folder holding the data itself. Neither is a folder to walk
//! into, so the bin is listed through this module rather than through the disk:
//! [`is_root`] recognises the one magic path the sidebar opens, [`list`] turns
//! the pairs into ordinary [`Entry`] rows, and [`restore`] and
//! [`delete_permanently`] are the only two things that can be done with one.
//!
//! The record comes in two shapes. Version 1 is a 280-byte structure written by
//! Windows 95 through 2000; version 2 is 800 bytes and written by everything
//! since Vista. Both start with the same three 8-byte fields - a version, the
//! deleted item's size and the deletion time as a `FILETIME` - followed by the
//! original path as UTF-16, so one reader covers both.
//!
//! The parsing and the moves are plain `std` and are kept off the Windows-only
//! drive scan so they can be exercised against a fake bin in a temp folder. Only
//! [`list`] needs the OS, to ask which drives are fixed; elsewhere it returns
//! nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::fs_model::{self, Entry, long_path};

/// The magic path the sidebar's Recycle Bin row points at. It names no folder
/// on any disk, which is what keeps it from colliding with a real one.
pub const ROOT: &str = "::Recycle::";

/// The most items one listing will build, so a bin holding a great many does
/// not turn into an unbounded amount of work.
const MAX_ITEMS: usize = 20_000;

/// Whether this is the magic Recycle Bin path.
pub fn is_root(path: &Path) -> bool {
    path.as_os_str() == std::ffi::OsStr::new(ROOT)
}

/// Whether a path is a `$R…` data file or `$I…` record inside a `$Recycle.Bin`.
///
/// The list uses this to offer Restore and Delete permanently instead of the
/// ordinary file operations, and to keep a double-click from opening the raw
/// data or walking into a deleted folder.
pub fn is_item(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let upper = name.to_ascii_uppercase();
    if !(upper.starts_with("$R") || upper.starts_with("$I")) {
        return false;
    }
    // The pair sits directly under the user's SID folder, which sits directly
    // under `$Recycle.Bin`; any ancestor being the bin is enough.
    path.ancestors().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("$Recycle.Bin"))
    })
}

/// What an `$I` record said about one deleted item.
#[derive(Clone, Debug)]
struct Deleted {
    size: u64,
    deleted: Option<SystemTime>,
    original: PathBuf,
}

/// Reads one `$I` record, or `None` when it is not one of the two known shapes.
///
/// A record of a different length or version is skipped rather than guessed at:
/// the layout beyond the three header fields depends on the version, so a
/// record that does not match would decode into a nonsense path.
fn parse_i(bytes: &[u8]) -> Option<Deleted> {
    let header = bytes.get(0..24)?;
    let version = u64::from_le_bytes(header[0..8].try_into().ok()?);
    let size = u64::from_le_bytes(header[8..16].try_into().ok()?);
    let filetime = u64::from_le_bytes(header[16..24].try_into().ok()?);
    // Version 1 is 280 bytes, version 2 is 800; anything else is not a record.
    match (version, bytes.len()) {
        (1, 280) | (2, 800) => {}
        _ => return None,
    }
    // The path is UTF-16 from just after the header to the end of the record,
    // NUL-padded. Stopping at the first NUL is what trims that padding, and the
    // remaining bytes differ between the two versions for the same reason.
    let mut units = Vec::new();
    for chunk in bytes[24..].as_chunks::<2>().0 {
        let unit = u16::from_le_bytes(*chunk);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    let original = PathBuf::from(String::from_utf16(&units).ok()?);
    if original.as_os_str().is_empty() {
        return None;
    }
    Some(Deleted {
        size,
        deleted: filetime_to_time(filetime),
        original,
    })
}

/// A `FILETIME` as a `SystemTime`, or `None` for the zero value Windows leaves
/// on an item whose deletion time it never recorded.
///
/// A `FILETIME` counts 100-nanosecond ticks from 1601-01-01, so the epoch
/// difference below is the number of ticks between then and 1970-01-01.
fn filetime_to_time(ft: u64) -> Option<SystemTime> {
    const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
    if ft == 0 {
        return None;
    }
    // A checked multiply, so a corrupt field can never overflow into a panic.
    if ft >= EPOCH_DIFF {
        let ticks = (ft - EPOCH_DIFF).checked_mul(100)?;
        Some(UNIX_EPOCH + Duration::from_nanos(ticks))
    } else {
        let ticks = (EPOCH_DIFF - ft).checked_mul(100)?;
        Some(UNIX_EPOCH - Duration::from_nanos(ticks))
    }
}

/// The `$R` and `$I` paths a data path or record path names.
fn pair(item: &Path) -> Option<(PathBuf, PathBuf)> {
    let dir = item.parent()?;
    let name = item.file_name()?.to_str()?;
    let suffix = name
        .strip_prefix("$R")
        .or_else(|| name.strip_prefix("$I"))?;
    Some((
        dir.join(format!("$R{suffix}")),
        dir.join(format!("$I{suffix}")),
    ))
}

/// One level of one `$Recycle.Bin\<SID>` folder: every `$I…` record that still
/// has its `$R…` data beside it, as an [`Entry`].
fn list_in(bin_dir: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    // An absent or unreadable bin is simply empty; a bin is not promised to
    // exist on every volume.
    let Ok(items) = std::fs::read_dir(long_path(bin_dir)) else {
        return out;
    };
    // One pass over the folder builds both sides at once: the data files' types,
    // and the record paths to read. Asking for each `$R` file's metadata per
    // record was one extra syscall per item, and a bin with many items made
    // opening it noticeably slow.
    let mut data: std::collections::HashMap<String, (bool, bool)> =
        std::collections::HashMap::new();
    let mut records: Vec<(String, PathBuf)> = Vec::new();
    for item in items.flatten() {
        let name = item.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(suffix) = name.strip_prefix("$R") {
            if let Ok(ft) = item.file_type() {
                data.insert(suffix.to_owned(), (ft.is_dir(), ft.is_symlink()));
            }
        } else if let Some(suffix) = name.strip_prefix("$I") {
            records.push((suffix.to_owned(), item.path()));
        }
    }
    for (suffix, record) in records {
        if out.len() >= MAX_ITEMS {
            break;
        }
        // An orphaned record, whose data file is gone, is skipped.
        let Some(&(is_dir, is_symlink)) = data.get(&suffix) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(long_path(&record)) else {
            continue;
        };
        let Some(deleted) = parse_i(&bytes) else {
            continue;
        };
        // The row reads as the original name; the path stays the real `$R` one,
        // because that is what restoring and deleting must act on.
        let display = deleted.original.file_name().map_or_else(
            || deleted.original.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        out.push(Entry {
            name: display,
            path: bin_dir.join(format!("$R{suffix}")),
            is_dir,
            is_symlink,
            size: deleted.size,
            modified: deleted.deleted,
            hidden: false,
        });
    }
    out
}

/// Every fixed drive's Recycle Bin, as one listing.
///
/// A deleted item keeps the drive it was deleted from, so every volume's bin is
/// read: there is no single folder that holds them all.
#[cfg(windows)]
pub fn list() -> Vec<Entry> {
    // A bin is read from every volume, and one spun-down or slow volume can
    // make the whole scan take a fifth of a second. A bin rarely changes, so a
    // reading is reused for a moment: re-opening it, or a listing refresh, is
    // then instant, and the worst case is a few seconds of staleness.
    const TTL: std::time::Duration = std::time::Duration::from_secs(2);
    if let Ok(g) = cache().lock()
        && let Some((at, list)) = &*g
        && at.elapsed() < TTL
    {
        return list.clone();
    }
    let fresh = scan();
    if let Ok(mut g) = cache().lock() {
        *g = Some((std::time::Instant::now(), fresh.clone()));
    }
    fresh
}

/// The last reading of the bins: when it was taken, and the listing.
#[cfg(windows)]
type BinSnapshot = (std::time::Instant, Vec<Entry>);

/// The last bin listing, shared by every caller.
#[cfg(windows)]
fn cache() -> &'static std::sync::Mutex<Option<BinSnapshot>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<BinSnapshot>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Reads every volume's bin now, touching the OS.
#[cfg(windows)]
fn scan() -> Vec<Entry> {
    // Every `<volume>\$Recycle.Bin\<SID>` folder, gathered first. They are
    // independent, so they are read at once rather than one after another: the
    // cost is the file opens, and overlapping them is most of the win on a bin
    // with many items.
    let mut bins: Vec<PathBuf> = Vec::new();
    for drive in fixed_drives() {
        let bin = drive.join("$Recycle.Bin");
        let Ok(sids) = std::fs::read_dir(long_path(&bin)) else {
            continue;
        };
        bins.extend(sids.flatten().map(|sid| sid.path()));
    }
    let mut out = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = bins
            .iter()
            .map(|bin| scope.spawn(move || list_in(bin)))
            .collect();
        for handle in handles {
            if let Ok(entries) = handle.join() {
                out.extend(entries);
            }
        }
    });
    out.truncate(MAX_ITEMS);
    out
}

/// There is no `$Recycle.Bin` off Windows, so the magic path lists as empty.
#[cfg(not(windows))]
pub fn list() -> Vec<Entry> {
    Vec::new()
}

/// The drive roots that are fixed disks, e.g. `C:\`.
#[cfg(windows)]
fn fixed_drives() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    // `DRIVE_FIXED`, from the Win32 API. Spelled out rather than imported so
    // that one constant does not pull in another windows-sys feature.
    const DRIVE_FIXED: u32 = 3;

    // SAFETY: takes no arguments and returns a bitmask; it touches nothing this
    // process owns.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        // `C:\` as a NUL-terminated UTF-16 string.
        let root = [letter as u16, u16::from(b':'), u16::from(b'\\'), 0];
        // SAFETY: `root` is a valid NUL-terminated UTF-16 string that is alive
        // for the whole call.
        if unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_FIXED {
            out.push(PathBuf::from(format!("{letter}:\\")));
        }
    }
    out
}

/// Restores the item at `item` (an `$R…` path) to where it was deleted from.
pub fn restore(item: &Path) -> Result<(), String> {
    let dir = item
        .parent()
        .ok_or_else(|| String::from("not a recycle item"))?;
    restore_in(dir, item)
}

/// Restores an item whose `$I`/`$R` pair lives in `bin_dir`, to where the record
/// says it came from.
///
/// The parent folders are made again if they are gone. A name that is already
/// taken at the destination is not overwritten: the item comes back beside it as
/// `name (2)`. The data is moved first and the record deleted only after that
/// succeeds, so a move that fails leaves the item in the bin to try again.
pub fn restore_in(bin_dir: &Path, item: &Path) -> Result<(), String> {
    let name = item
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| String::from("not a recycle item"))?;
    let suffix = name
        .strip_prefix("$R")
        .or_else(|| name.strip_prefix("$I"))
        .ok_or_else(|| String::from("not a recycle item"))?;
    let i_path = bin_dir.join(format!("$I{suffix}"));
    let r_path = bin_dir.join(format!("$R{suffix}"));

    let bytes = std::fs::read(long_path(&i_path))
        .map_err(|e| format!("cannot read {}: {e}", i_path.display()))?;
    let deleted = parse_i(&bytes)
        .ok_or_else(|| format!("{} is not a readable recycle record", i_path.display()))?;

    if let Some(parent) = deleted.original.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(long_path(parent))
            .map_err(|e| format!("cannot make {}: {e}", parent.display()))?;
    }
    let dest = fs_model::unique_dest(&deleted.original);
    std::fs::rename(long_path(&r_path), long_path(&dest))
        .map_err(|e| format!("cannot move {} back: {e}", r_path.display()))?;
    std::fs::remove_file(long_path(&i_path)).map_err(|e| {
        format!(
            "restored, but {} could not be removed: {e}",
            i_path.display()
        )
    })?;
    Ok(())
}

/// Removes an item for good: the `$R…` data, file or folder, and then its
/// `$I…` record.
pub fn delete_permanently(item: &Path) -> Result<(), String> {
    let (r_path, i_path) = pair(item).ok_or_else(|| String::from("not a recycle item"))?;
    let md = std::fs::symlink_metadata(long_path(&r_path))
        .map_err(|e| format!("cannot read {}: {e}", r_path.display()))?;
    let removed = if md.is_dir() {
        std::fs::remove_dir_all(long_path(&r_path))
    } else {
        std::fs::remove_file(long_path(&r_path))
    };
    removed.map_err(|e| format!("cannot delete {}: {e}", r_path.display()))?;
    std::fs::remove_file(long_path(&i_path)).map_err(|e| {
        format!(
            "deleted the data, but {} could not be removed: {e}",
            i_path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("rhumb-recycle-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A hand-built `$I` record: the three header fields, then the original path
    /// as UTF-16, zero-padded to `total` bytes.
    fn record(version: u64, size: u64, filetime: u64, path: &str, total: usize) -> Vec<u8> {
        let mut b = vec![0u8; total];
        b[0..8].copy_from_slice(&version.to_le_bytes());
        b[8..16].copy_from_slice(&size.to_le_bytes());
        b[16..24].copy_from_slice(&filetime.to_le_bytes());
        for (i, unit) in path.encode_utf16().enumerate() {
            let at = 24 + i * 2;
            if at + 2 > total {
                break;
            }
            b[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        b
    }

    /// 2023-11-14 22:13:20 UTC, as a `FILETIME`.
    const FILETIME: u64 = 133_444_736_000_000_000;

    #[test]
    fn a_version_one_record_parses() {
        let bytes = record(1, 1234, FILETIME, r"C:\Users\dev\notes.txt", 280);
        let d = parse_i(&bytes).unwrap();
        assert_eq!(d.size, 1234);
        assert_eq!(d.original, PathBuf::from(r"C:\Users\dev\notes.txt"));
        assert_eq!(
            d.deleted,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
    }

    #[test]
    fn a_version_two_record_parses() {
        let bytes = record(2, 5678, FILETIME, r"C:\Users\dev\photo.jpg", 800);
        let d = parse_i(&bytes).unwrap();
        assert_eq!(d.size, 5678);
        assert_eq!(d.original, PathBuf::from(r"C:\Users\dev\photo.jpg"));
        assert_eq!(
            d.deleted,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
    }

    #[test]
    fn the_path_stops_at_the_first_nul() {
        // Only the name is written; everything after it stays zero.
        let bytes = record(2, 9, FILETIME, "C:\\gone.txt", 800);
        let d = parse_i(&bytes).unwrap();
        assert_eq!(d.original, PathBuf::from("C:\\gone.txt"));
    }

    #[test]
    fn a_wrong_version_or_size_is_skipped() {
        // An unknown version.
        assert!(parse_i(&record(3, 1, FILETIME, r"C:\x", 800)).is_none());
        // A version-1 record of the wrong length.
        assert!(parse_i(&record(1, 1, FILETIME, r"C:\x", 300)).is_none());
        // A version-2 record of the wrong length.
        assert!(parse_i(&record(2, 1, FILETIME, r"C:\x", 280)).is_none());
        // Too short to hold even a header.
        assert!(parse_i(&[0u8; 8]).is_none());
    }

    #[test]
    fn a_zero_deletion_time_is_no_time() {
        let bytes = record(2, 1, 0, r"C:\x", 800);
        assert!(parse_i(&bytes).unwrap().deleted.is_none());
    }

    #[test]
    fn is_root_only_matches_the_magic_path() {
        assert!(is_root(Path::new(ROOT)));
        assert!(!is_root(Path::new(r"C:\")));
        assert!(!is_root(Path::new(r"::Recycle::\x")));
        assert!(!is_root(Path::new("")));
    }

    #[test]
    fn is_item_finds_records_and_data_under_a_bin() {
        let bin = Path::new("C:").join("$Recycle.Bin").join("S-1-5-21-1");
        assert!(is_item(&bin.join("$RABCDEF.txt")));
        assert!(is_item(&bin.join("$IABCDEF.txt")));
        // Not a record, even inside the bin.
        assert!(!is_item(&bin.join("desktop.ini")));
        // A record-looking name outside any bin is not one.
        let outside = Path::new("C:").join("Users").join("dev");
        assert!(!is_item(&outside.join("$RABCDEF.txt")));
    }

    #[test]
    fn listing_an_empty_or_missing_bin_is_empty() {
        let d = dir("empty");
        assert!(list_in(&d.join("missing")).is_empty());
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(list_in(&empty).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn listing_a_bin_pairs_records_with_their_data() {
        let d = dir("pair");
        let bin = d.join("$Recycle.Bin").join("SID");
        std::fs::create_dir_all(&bin).unwrap();
        let original = d.join("gone.txt");
        std::fs::write(bin.join("$RABC.txt"), b"hello").unwrap();
        std::fs::write(
            bin.join("$IABC.txt"),
            record(2, 5, FILETIME, &original.to_string_lossy(), 800),
        )
        .unwrap();
        // An orphaned record with no data is not shown.
        std::fs::write(
            bin.join("$IORPHAN.txt"),
            record(2, 1, FILETIME, r"C:\x", 800),
        )
        .unwrap();

        let entries = list_in(&bin);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].name, "gone.txt");
        assert_eq!(entries[0].path, bin.join("$RABC.txt"));
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, 5);
        assert_eq!(
            entries[0].modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restore_puts_the_data_back_and_drops_the_record() {
        let d = dir("restore");
        let bin = d.join("$Recycle.Bin").join("SID");
        std::fs::create_dir_all(&bin).unwrap();
        // The original folder is gone too, so it has to be made again.
        let original = d.join("restored").join("deep").join("hello.txt");
        std::fs::write(bin.join("$RABCDEF.txt"), b"hi there").unwrap();
        std::fs::write(
            bin.join("$IABCDEF.txt"),
            record(2, 8, FILETIME, &original.to_string_lossy(), 800),
        )
        .unwrap();

        restore_in(&bin, &bin.join("$RABCDEF.txt")).unwrap();

        assert_eq!(std::fs::read(&original).unwrap(), b"hi there");
        assert!(!bin.join("$RABCDEF.txt").exists(), "the data moved");
        assert!(!bin.join("$IABCDEF.txt").exists(), "the record is gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restoring_over_a_taken_name_keeps_both() {
        let d = dir("collide");
        let bin = d.join("$Recycle.Bin").join("SID");
        std::fs::create_dir_all(&bin).unwrap();
        let original = d.join("out").join("hello.txt");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, b"old").unwrap();
        std::fs::write(bin.join("$RABCDEF.txt"), b"new").unwrap();
        std::fs::write(
            bin.join("$IABCDEF.txt"),
            record(2, 3, FILETIME, &original.to_string_lossy(), 800),
        )
        .unwrap();

        restore_in(&bin, &bin.join("$RABCDEF.txt")).unwrap();

        assert_eq!(std::fs::read(&original).unwrap(), b"old", "left untouched");
        let beside = original.parent().unwrap().join("hello (2).txt");
        assert_eq!(std::fs::read(&beside).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn delete_permanently_removes_data_and_record() {
        let d = dir("delete");
        let bin = d.join("$Recycle.Bin").join("SID");
        std::fs::create_dir_all(&bin).unwrap();
        // A folder item: the whole tree must go.
        std::fs::create_dir_all(bin.join("$RDIR").join("inner")).unwrap();
        std::fs::write(bin.join("$RDIR").join("inner").join("f.txt"), b"x").unwrap();
        std::fs::write(
            bin.join("$IDIR"),
            record(2, 0, FILETIME, r"C:\gone-folder", 800),
        )
        .unwrap();

        delete_permanently(&bin.join("$RDIR")).unwrap();

        assert!(!bin.join("$RDIR").exists());
        assert!(!bin.join("$IDIR").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_bad_path_is_an_error_and_not_a_panic() {
        let d = dir("bad");
        assert!(restore_in(&d, &d.join("plain.txt")).is_err());
        assert!(delete_permanently(&d.join("plain.txt")).is_err());
        assert!(restore(&d).is_err(), "a folder with no parent name");
        let _ = std::fs::remove_dir_all(&d);
    }
}
