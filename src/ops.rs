//! File operations: copy, move and delete, all on background threads with
//! progress, cancellation and non-destructive conflict handling.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[cfg(test)]
use crate::fs_model::{self, free_space};
use crate::workers::{Job, Msg, OpKind, Outcome, Progress};

const CHUNK: usize = 512 * 1024;

/// What the copy/paste clipboard holds.
#[derive(Clone, Debug)]
pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    /// `true` = cut (move on paste), `false` = copy.
    pub cut: bool,
}

/// Starts a copy or move where every destination is already resolved, which is
/// what the UI does so it can record an exact undo mapping.
pub fn start_transfer_pairs(
    tx: Sender<Msg>,
    id: u64,
    pairs: Vec<(PathBuf, PathBuf)>,
    cut: bool,
) -> Job {
    let sources: Vec<PathBuf> = pairs.iter().map(|(s, _)| s.clone()).collect();
    let total_items = sources.len();
    let total_bytes: u64 = sources.iter().map(|p| dir_size(p)).sum();
    let cancel = Arc::new(AtomicBool::new(false));
    let verb = if cut { "Moving" } else { "Copying" };
    let job = Job {
        id,
        kind: if cut { OpKind::Move } else { OpKind::Copy },
        label: format!(
            "{verb} {total_items} item{}",
            if total_items == 1 { "" } else { "s" }
        ),
        cancel: cancel.clone(),
        started: std::time::Instant::now(),
    };

    std::thread::Builder::new()
        .name("xplor-op".into())
        .spawn(move || {
            let mut progress = Progress {
                id,
                done_items: 0,
                total_items,
                done_bytes: 0,
                total_bytes,
                current: String::new(),
                failed: Vec::new(),
            };
            let _ = tx.send(Msg::Progress(progress.clone()));
            let mut ok = 0usize;
            let mut failed: Vec<String> = Vec::new();
            for (n, (src, dest)) in pairs.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(Msg::Finished {
                        id,
                        outcome: Outcome::Cancelled { done: ok },
                    });
                    return;
                }
                progress.current = short(src);
                let r = if cut {
                    move_path(src, dest, &tx, &mut progress, &cancel)
                } else {
                    copy_path(src, dest, &tx, &mut progress, &cancel)
                };
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push(format!("{}: {e}", short(src))),
                }
                progress.done_items = n + 1;
                let _ = tx.send(Msg::Progress(progress.clone()));
            }
            let _ = tx.send(Msg::Finished {
                id,
                outcome: Outcome::Done { ok, failed },
            });
        })
        .expect("spawn transfer thread");
    job
}

/// Starts a copy or move job for `sources` into `dest_dir`.
#[cfg(test)]
pub fn start_transfer(
    tx: Sender<Msg>,
    id: u64,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    cut: bool,
) -> Job {
    let cancel = Arc::new(AtomicBool::new(false));
    let label = format!(
        "{} {} item{}",
        if cut { "Moving" } else { "Copying" },
        sources.len(),
        if sources.len() == 1 { "" } else { "s" }
    );
    let job = Job {
        id,
        kind: if cut { OpKind::Move } else { OpKind::Copy },
        label,
        cancel: cancel.clone(),
        started: std::time::Instant::now(),
    };

    let spawned = Job {
        id,
        kind: job.kind,
        label: job.label.clone(),
        cancel: cancel.clone(),
        started: job.started,
    };

    std::thread::Builder::new()
        .name("xplor-op".into())
        .spawn(move || run_transfer(tx, id, sources, dest_dir, cut, cancel))
        .expect("spawn transfer thread");

    spawned
}

#[cfg(test)]
fn run_transfer(
    tx: Sender<Msg>,
    id: u64,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    cut: bool,
    cancel: Arc<AtomicBool>,
) {
    let total_items = sources.len();
    let total_bytes: u64 = sources.iter().map(|p| dir_size(p)).sum();

    let mut progress = Progress {
        id,
        done_items: 0,
        total_items,
        done_bytes: 0,
        total_bytes,
        current: String::new(),
        failed: Vec::new(),
    };
    let _ = tx.send(Msg::Progress(progress.clone()));

    // Moving onto another volume cannot be a rename; warn only if space is short.
    if total_bytes > 0 && !cut {
        if let Some((avail, _)) = free_space(&dest_dir) {
            if avail.saturating_sub(16 * 1024 * 1024) < total_bytes {
                let msg = format!(
                    "Not enough free space in {} (need {}, have {})",
                    short(&dest_dir),
                    fs_model::fmt_size(total_bytes),
                    fs_model::fmt_size(avail)
                );
                progress.failed.push(msg.clone());
                let _ = tx.send(Msg::Progress(progress));
                let _ = tx.send(Msg::Finished {
                    id,
                    outcome: Outcome::Failed(msg),
                });
                return;
            }
        }
    }

    let mut ok = 0usize;
    let mut failed: Vec<String> = progress.failed.clone();

    for (i, src) in sources.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Msg::Finished {
                id,
                outcome: Outcome::Cancelled { done: ok },
            });
            return;
        }
        let file_name = src
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut dest = dest_dir.join(&file_name);
        // Copying a folder into itself would recurse forever.
        if src.is_dir() && normalize(&dest_dir).starts_with(normalize(src)) {
            failed.push(format!(
                "Skipped {}: cannot copy a folder into itself",
                file_name
            ));
            progress.done_items = i + 1;
            progress.current = file_name;
            progress.failed = failed.clone();
            let _ = tx.send(Msg::Progress(progress.clone()));
            continue;
        }
        if dest == *src {
            dest = fs_model::unique_dest(&fs_model::unique_dest(&dest));
        } else if dest.exists() {
            dest = fs_model::unique_dest(&dest);
        }

        let result = if cut {
            move_path(src, &dest, &tx, &mut progress, &cancel)
        } else {
            copy_path(src, &dest, &tx, &mut progress, &cancel)
        };

        match result {
            Ok(()) => ok += 1,
            Err(e) => failed.push(format!("{file_name}: {e}")),
        }
        progress.done_items = i + 1;
        progress.current = file_name;
        progress.failed = failed.clone();
        let _ = tx.send(Msg::Progress(progress.clone()));
    }

    if cancel.load(Ordering::Relaxed) {
        let _ = tx.send(Msg::Finished {
            id,
            outcome: Outcome::Cancelled { done: ok },
        });
    } else {
        let _ = tx.send(Msg::Finished {
            id,
            outcome: Outcome::Done { ok, failed },
        });
    }
}

/// The canonical form of a path, for the "cannot move a folder into itself" check.
#[cfg(test)]
fn normalize(p: &Path) -> PathBuf {
    fs_model::normalize(p)
}

fn short(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string())
}

/// Total bytes under a path (file size, or recursive sum for folders).
pub fn dir_size(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
        if let Ok(md) = entry.metadata() {
            if md.is_file() {
                total += md.len();
            }
        }
    }
    total
}

/// Counts items (files + folders) under a path, for progress totals.
fn count_items(path: &Path) -> usize {
    if path.is_file() {
        return 1;
    }
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter(|e| e.depth() == 0 || true)
        .count()
}

fn copy_path(
    src: &Path,
    dest: &Path,
    tx: &Sender<Msg>,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in walkdir::WalkDir::new(src).min_depth(1).sort_by_file_name() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let entry = entry.map_err(|e| io::Error::other(e.to_string()))?;
            let rel = entry
                .path()
                .strip_prefix(src)
                .map_err(|e| io::Error::other(e.to_string()))?;
            let target = dest.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                let name = rel.display().to_string();
                progress.current = name.clone();
                copy_file(entry.path(), &target, tx, progress, cancel)?;
                if let Ok(md) = entry.metadata() {
                    let _ = copy_times(&target, &md);
                }
            }
        }
        Ok(())
    } else {
        let name = src
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        progress.current = name;
        copy_file(src, dest, tx, progress, cancel)
    }
}

/// Writes `sources` into a new zip beside them, the way Explorer's
/// "Compressed (zipped) folder" does.
///
/// The archive name comes from the first item, so compressing a folder called
/// `photos` produces `photos.zip`, and compressing a mixed selection produces
/// `<name>.zip` from the first entry.
pub fn start_zip(
    tx: Sender<Msg>,
    id: u64,
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    cancel: Arc<AtomicBool>,
) -> Job {
    let label = match sources.first() {
        Some(p) => {
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "archive".to_owned());
            format!("Compressing to {stem}.zip")
        }
        None => "Compressing".to_owned(),
    };
    let job = Job {
        id,
        kind: OpKind::Compress,
        label,
        cancel: cancel.clone(),
        started: std::time::Instant::now(),
    };
    let archive_name = archive_name_for(&sources);
    let dest = dest_dir.join(&archive_name);

    std::thread::Builder::new()
        .name("xplor-zip".into())
        .spawn(move || {
            // Counting first means the progress bar is honest from the start.
            let mut total_items = 0usize;
            let mut total_bytes = 0u64;
            for src in &sources {
                if src.is_dir() {
                    for entry in walkdir::WalkDir::new(src).min_depth(1) {
                        let Ok(entry) = entry else { continue };
                        if entry.file_type().is_file() {
                            total_items += 1;
                            total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                        }
                    }
                } else {
                    total_items += 1;
                    total_bytes += src.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
            let mut progress = Progress {
                id,
                done_items: 0,
                total_items,
                done_bytes: 0,
                total_bytes,
                current: archive_name.clone(),
                failed: Vec::new(),
            };
            let _ = tx.send(Msg::Progress(progress.clone()));
            match write_zip(&sources, &dest, &tx, &mut progress, &cancel) {
                Ok(()) => {
                    let _ = tx.send(Msg::Finished {
                        id,
                        outcome: Outcome::Done {
                            ok: total_items,
                            failed: Vec::new(),
                        },
                    });
                }
                Err(e) => {
                    // A half-written archive is worse than none at all.
                    let _ = fs::remove_file(&dest);
                    let _ = tx.send(Msg::Finished {
                        id,
                        outcome: Outcome::Failed(e),
                    });
                }
            }
        })
        .expect("spawn zip thread");
    job
}

/// The archive name for a selection, avoiding collisions with what is there.
fn archive_name_for(sources: &[PathBuf]) -> String {
    let stem = sources
        .first()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_owned());
    format!("{stem}.zip")
}

/// Adds every file under `sources` to a new zip at `dest`.
///
/// `progress.total_items` is expected to be the file count, which the worker
/// fills in before it starts writing.
fn write_zip(
    sources: &[PathBuf],
    dest: &Path,
    tx: &Sender<Msg>,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<(), String> {
    use zip::write::SimpleFileOptions;

    let file = fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for src in sources {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let base = src.file_name().unwrap_or_default().to_os_string();
        let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
        if src.is_dir() {
            for entry in walkdir::WalkDir::new(src).min_depth(1).sort_by_file_name() {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().is_file() {
                    let rel = entry.path().strip_prefix(src).map_err(|e| e.to_string())?;
                    files.push((entry.path().to_path_buf(), Path::new(&base).join(rel)));
                }
            }
        } else {
            files.push((src.clone(), PathBuf::from(&base)));
        }

        for (from, to) in &files {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            progress.current = short(from);
            let mut r = fs::File::open(from).map_err(|e| format!("{}: {e}", short(from)))?;
            // Names inside a zip always use forward slashes.
            let name = to.to_string_lossy().replace('\\', "/");
            zip.start_file(name, options)
                .map_err(|e| format!("{}: {e}", short(from)))?;
            let mut buf = vec![0u8; CHUNK];
            let mut last_report = std::time::Instant::now();
            loop {
                let read = r.read(&mut buf).map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                zip.write_all(&buf[..read]).map_err(|e| e.to_string())?;
                progress.done_bytes += read as u64;
                if last_report.elapsed().as_millis() > 60 {
                    last_report = std::time::Instant::now();
                    let _ = tx.send(Msg::Progress(progress.clone()));
                }
            }
            progress.done_items += 1;
            let _ = tx.send(Msg::Progress(progress.clone()));
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn copy_file(
    src: &Path,
    dest: &Path,
    tx: &Sender<Msg>,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut r = fs::File::open(src)?;
    let mut w = fs::File::create(dest)?;
    let mut buf = vec![0u8; CHUNK];
    let mut last_report = std::time::Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        progress.done_bytes += n as u64;
        // Throttle updates so the channel never floods.
        if last_report.elapsed().as_millis() > 60 {
            last_report = std::time::Instant::now();
            let _ = tx.send(Msg::Progress(progress.clone()));
        }
    }
    w.flush()?;
    Ok(())
}

/// Best effort: keep the modification time of the original.
fn copy_times(dest: &Path, md: &fs::Metadata) -> io::Result<()> {
    let file = fs::File::options().write(true).open(dest)?;
    file.set_modified(md.modified()?)
}

fn move_path(
    src: &Path,
    dest: &Path,
    tx: &Sender<Msg>,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> io::Result<()> {
    // Fast path: same volume, no data copy needed.
    match fs::rename(src, dest) {
        Ok(()) => {
            progress.done_bytes += dir_size(src);
            return Ok(());
        }
        Err(_) => { /* cross-device: fall through to copy + delete */ }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_path(src, dest, tx, progress, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    if src.is_dir() {
        fs::remove_dir_all(src)
    } else {
        fs::remove_file(src)
    }
}

/// Moves one item synchronously, for undo where the work must complete
/// before the next frame. Falls back to a real copy when the move crosses
/// volumes.
pub fn move_now(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    let (tx, rx) = crate::workers::bus();
    let cancel = AtomicBool::new(false);
    let mut progress = Progress {
        id: 0,
        done_items: 0,
        total_items: 0,
        done_bytes: 0,
        total_bytes: 0,
        current: String::new(),
        failed: Vec::new(),
    };
    if src.is_dir() {
        fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        for entry in walkdir::WalkDir::new(src).min_depth(1).sort_by_file_name() {
            let entry = entry.map_err(|e| e.to_string())?;
            let rel = entry.path().strip_prefix(src).map_err(|e| e.to_string())?;
            let target = dest.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            } else {
                copy_file(entry.path(), &target, &tx, &mut progress, &cancel)
                    .map_err(|e| e.to_string())?;
            }
        }
        fs::remove_dir_all(src).map_err(|e| e.to_string())?;
    } else {
        copy_file(src, dest, &tx, &mut progress, &cancel).map_err(|e| e.to_string())?;
        fs::remove_file(src).map_err(|e| e.to_string())?;
    }
    let _ = rx.try_recv();
    Ok(())
}

/// Reads the head of a text file for the details pane.
///
/// Capped so a huge file costs the same as a small one, and lossy so a stray
/// byte cannot fail the whole read.
pub fn peek_text(path: &Path) -> Option<String> {
    /// Enough for a screenful of preview lines.
    const CAP: usize = 8 * 1024;
    let mut buf = Vec::with_capacity(CAP);
    let file = fs::File::open(path).ok()?;
    // Read one byte past the cap so a cut multi-byte character is detectable.
    file.take((CAP + 1) as u64).read_to_end(&mut buf).ok()?;
    if buf.len() > CAP {
        buf.truncate(CAP);
    }
    let text = String::from_utf8_lossy(&buf);
    // A file with no line breaks would be one very long line; cut it too.
    let mut out: String = text.chars().take(4000).collect();
    if out.len() < text.len() {
        out.push('\u{2026}');
    }
    Some(out)
}

/// Sends items to the OS recycle bin / trash. Nothing is destroyed.
pub fn send_to_trash(paths: &[PathBuf]) -> Result<(), String> {
    // A vanished file is not an error: the goal (it is gone) is met.
    let existing: Vec<PathBuf> = paths.iter().filter(|p| p.exists()).cloned().collect();
    if existing.is_empty() {
        return Err("Nothing to delete".into());
    }
    trash::delete_all(&existing).map_err(|e| e.to_string())
}

/// Permanently deletes, on a worker thread, with progress and cancellation.
pub fn start_permanent_delete(
    tx: Sender<Msg>,
    id: u64,
    paths: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
) -> Job {
    let job = Job {
        id,
        kind: OpKind::Delete,
        label: format!(
            "Deleting {} item{}",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ),
        cancel: cancel.clone(),
        started: std::time::Instant::now(),
    };
    let spawned = Job {
        id,
        kind: job.kind,
        label: job.label.clone(),
        cancel: cancel.clone(),
        started: job.started,
    };
    std::thread::Builder::new()
        .name("xplor-del".into())
        .spawn(move || {
            let total_items: usize = paths.iter().map(|p| count_items(p)).sum();
            let total_bytes: u64 = paths.iter().map(|p| dir_size(p)).sum();
            let mut progress = Progress {
                id,
                done_items: 0,
                total_items: total_items.max(paths.len()),
                done_bytes: 0,
                total_bytes,
                current: String::new(),
                failed: Vec::new(),
            };
            let _ = tx.send(Msg::Progress(progress.clone()));
            let mut ok = 0usize;
            let mut failed = Vec::new();
            for p in &paths {
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(Msg::Finished {
                        id,
                        outcome: Outcome::Cancelled { done: ok },
                    });
                    return;
                }
                let r = if p.is_dir() {
                    fs::remove_dir_all(p)
                } else {
                    fs::remove_file(p)
                };
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push(format!("{}: {e}", short(p))),
                }
                progress.done_items += 1;
                let _ = tx.send(Msg::Progress(progress.clone()));
            }
            let _ = tx.send(Msg::Finished {
                id,
                outcome: Outcome::Done { ok, failed },
            });
        })
        .expect("spawn delete thread");
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::bus;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xplor-op-{name}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn copy_directory_tree() {
        let root = tmp("copy");
        let src = root.join("src");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("a.txt"), "alpha").unwrap();
        fs::write(src.join("nested/b.txt"), "beta").unwrap();

        let (tx, rx) = bus();
        start_transfer(tx, 1, vec![src.clone()], root.join("dst"), false);

        // Wait for the Finished message.
        let outcome = loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(Msg::Finished { outcome, .. }) => break outcome,
                Ok(_) => {}
                Err(e) => panic!("copy job stalled: {e}"),
            }
        };
        match outcome {
            Outcome::Done { ok, failed } => {
                assert_eq!(ok, 1, "failed: {failed:?}");
                assert!(failed.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(root.join("dst/src/nested/b.txt")).unwrap(),
            "beta"
        );
        assert_eq!(
            fs::read_to_string(root.join("dst/src/a.txt")).unwrap(),
            "alpha"
        );
        assert!(src.exists(), "source must survive a copy");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn move_removes_source() {
        let root = tmp("move");
        let src = root.join("a.txt");
        fs::write(&src, "hello").unwrap();
        let (tx, rx) = bus();
        start_transfer(tx, 1, vec![src.clone()], root.join("into"), true);
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(Msg::Finished { outcome, .. }) => {
                    assert!(
                        matches!(outcome, Outcome::Done { ok: 1, .. }),
                        "{outcome:?}"
                    );
                    break;
                }
                Ok(_) => {}
                Err(e) => panic!("move job stalled: {e}"),
            }
        }
        assert!(!src.exists());
        assert!(root.join("into/a.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_does_not_overwrite_existing_files() {
        let root = tmp("conflict");
        let src = root.join("a.txt");
        fs::write(&src, "new").unwrap();
        fs::create_dir_all(root.join("dst")).unwrap();
        fs::write(root.join("dst/a.txt"), "old").unwrap();
        let (tx, rx) = bus();
        start_transfer(tx, 1, vec![src], root.join("dst"), false);
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(Msg::Finished { .. }) => break,
                Ok(_) => {}
                Err(e) => panic!("stalled: {e}"),
            }
        }
        assert_eq!(
            fs::read_to_string(root.join("dst/a.txt")).unwrap(),
            "old",
            "existing file must not be clobbered"
        );
        assert_eq!(
            fs::read_to_string(root.join("dst/a (2).txt")).unwrap(),
            "new"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dir_size_counts_recursively() {
        let root = tmp("size");
        fs::create_dir_all(root.join("d")).unwrap();
        fs::write(root.join("d/x"), vec![0u8; 10]).unwrap();
        assert_eq!(dir_size(&root), 10);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn zip_keeps_the_folder_tree_and_names() {
        let root = tmp("zip");
        let src = root.join("photos");
        fs::create_dir_all(src.join("2024")).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("2024/b.txt"), b"nested").unwrap();

        let (tx, rx) = bus();
        let job = start_zip(
            tx,
            1,
            vec![src.clone()],
            root.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(job.kind, OpKind::Compress);
        // Drive the worker to completion by draining its messages.
        let mut finished = None;
        for _ in 0..200 {
            match rx.try_recv() {
                Ok(Msg::Finished { outcome, .. }) => {
                    finished = Some(outcome);
                    break;
                }
                Ok(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        let archive = root.join("photos.zip");
        assert!(archive.exists(), "no archive written: {finished:?}");

        let f = fs::File::open(&archive).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().to_owned())
            .collect();
        assert!(names.contains(&"photos/a.txt".to_owned()), "{names:?}");
        assert!(names.contains(&"photos/2024/b.txt".to_owned()), "{names:?}");
        // No Windows separators inside the archive.
        assert!(names.iter().all(|n| !n.contains('\\')), "{names:?}");

        let mut a = z.by_name("photos/2024/b.txt").unwrap();
        let mut body = String::new();
        a.read_to_string(&mut body).unwrap();
        assert_eq!(body, "nested");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_name_comes_from_the_first_item() {
        assert_eq!(
            archive_name_for(&[PathBuf::from("/a/photos")]),
            "photos.zip"
        );
        assert_eq!(
            archive_name_for(&[PathBuf::from("/a/report.txt")]),
            "report.zip"
        );
        assert_eq!(archive_name_for(&[]), "archive.zip");
    }
}
