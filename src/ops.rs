//! File operations: copy, move and delete, all on background threads with
//! progress, cancellation and non-destructive conflict handling.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[cfg(test)]
use crate::fs_model;
use crate::fs_model::long_path;
use crate::workers::{Job, Msg, OpKind, Outcome, Progress};

const CHUNK: usize = 512 * 1024;
/// How many times an operation that is momentarily locked by someone else is
/// tried again before the error is reported.
const RETRY_ATTEMPTS: u32 = 5;
/// The first nap between retries, in milliseconds. It doubles each time, so a
/// lock that clears at once costs almost nothing and a long one is not polled
/// hard.
const RETRY_FIRST_MS: u64 = 10;

/// A file-operation failure, classified so the retry logic and the log line do
/// not have to read the OS message to know what went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpError {
    /// The file is locked by another process for the moment.
    Locked,
    NotFound,
    Permission,
    /// The destination already exists.
    Exists,
    /// There is not enough room.
    NoSpace,
    /// Anything else, with the caller keeping the raw message.
    Other,
}

impl OpError {
    /// Classifies a raw OS error. The numeric codes are Windows' (32/33 sharing
    /// and lock violations, 2 not found, 5 access denied, 183 already exists,
    /// 112 disk full); on other platforms the `ErrorKind` fallback applies.
    pub fn from_io(e: &io::Error) -> OpError {
        match e.raw_os_error() {
            Some(32) | Some(33) => OpError::Locked,
            Some(2) => OpError::NotFound,
            Some(5) => OpError::Permission,
            Some(183) => OpError::Exists,
            Some(112) => OpError::NoSpace,
            _ => match e.kind() {
                io::ErrorKind::NotFound => OpError::NotFound,
                io::ErrorKind::PermissionDenied => OpError::Permission,
                io::ErrorKind::AlreadyExists => OpError::Exists,
                _ => OpError::Other,
            },
        }
    }

    /// Whether trying again shortly might succeed.
    pub fn is_retryable(self) -> bool {
        matches!(self, OpError::Locked)
    }

    /// A short, stable code, for a log line or an error report.
    pub fn code(self) -> &'static str {
        match self {
            OpError::Locked => "locked",
            OpError::NotFound => "not-found",
            OpError::Permission => "permission",
            OpError::Exists => "exists",
            OpError::NoSpace => "no-space",
            OpError::Other => "other",
        }
    }
}

/// Whether an error is the OS saying the file is locked by another process for
/// a moment.
fn is_sharing_violation(e: &io::Error) -> bool {
    OpError::from_io(e).is_retryable()
}

/// Runs `f`, trying again a few times when the OS says the file is briefly
/// locked by another process.
///
/// This is what stops an antivirus scanner or an open editor from turning a
/// copy into a spurious failure: the lock is usually gone within a few tens of
/// milliseconds, so a short nap and a retry is enough.
fn retrying<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut wait = RETRY_FIRST_MS;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < RETRY_ATTEMPTS && is_sharing_violation(&e) => {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                wait = wait.saturating_mul(2);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Blocks a worker while `paused` is set.
///
/// Naps rather than waiting on a condition variable: the flag is a plain atomic
/// the UI writes and there is no other thread to signal, and a 50 ms nap keeps
/// the thread at nothing while still noticing a resume or a cancel promptly.
fn wait_while_paused(paused: &AtomicBool, cancel: &AtomicBool) {
    while paused.load(Ordering::Relaxed) {
        // A pause must never make a cancel unreachable.
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

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
    let paused = Arc::new(AtomicBool::new(false));
    let verb = if cut { "Moving" } else { "Copying" };
    let job = Job {
        id,
        kind: if cut { OpKind::Move } else { OpKind::Copy },
        label: format!(
            "{verb} {total_items} item{}",
            if total_items == 1 { "" } else { "s" }
        ),
        cancel: cancel.clone(),
        paused: paused.clone(),
        started: std::time::Instant::now(),
    };

    std::thread::Builder::new()
        .name("rhumb-op".into())
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
                wait_while_paused(&paused, &cancel);
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(Msg::Finished {
                        id,
                        outcome: Outcome::Cancelled { done: ok },
                    });
                    return;
                }
                progress.current = short(src);
                let r = if cut {
                    move_path(src, dest, &tx, &mut progress, &cancel, &paused)
                } else {
                    copy_path(src, dest, &tx, &mut progress, &cancel, &paused)
                };
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => {
                        log::debug!(
                            "transfer failed [{}] {}: {e}",
                            OpError::from_io(&e).code(),
                            short(src)
                        );
                        failed.push(format!("{}: {e}", short(src)));
                    }
                }
                progress.done_items = n + 1;
                let _ = tx.send(Msg::Progress(progress.clone()));
            }
            let _ = tx.send(Msg::Finished {
                id,
                outcome: Outcome::Done { ok, failed },
            });
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the transfer worker: {e}"));
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
    let paused = Arc::new(AtomicBool::new(false));
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
        paused: paused.clone(),
        started: std::time::Instant::now(),
    };

    let spawned = Job {
        id,
        kind: job.kind,
        label: job.label.clone(),
        cancel: cancel.clone(),
        paused: paused.clone(),
        started: job.started,
    };

    std::thread::Builder::new()
        .name("rhumb-op".into())
        .spawn(move || run_transfer(tx, id, sources, dest_dir, cut, cancel, paused))
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
    paused: Arc<AtomicBool>,
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
    if total_bytes > 0
        && !cut
        && let Some((avail, _)) = fs_model::FreeSpace::blocking(&dest_dir)
        && avail.saturating_sub(16 * 1024 * 1024) < total_bytes
    {
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

    let mut ok = 0usize;
    let mut failed: Vec<String> = progress.failed.clone();

    for (i, src) in sources.iter().enumerate() {
        wait_while_paused(&paused, &cancel);
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
            move_path(src, &dest, &tx, &mut progress, &cancel, &paused)
        } else {
            copy_path(src, &dest, &tx, &mut progress, &cancel, &paused)
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

/// Bytes per second and the time left, from the bytes done so far.
///
/// `None` until there is a total and enough elapsed time for the estimate to be
/// worth showing: a rate read from a few milliseconds swings wildly and would
/// read as a lie. A job that is already there, or has no total, has nothing to
/// estimate either.
pub fn rate_and_eta(
    done: u64,
    total: u64,
    elapsed: std::time::Duration,
) -> Option<(f64, std::time::Duration)> {
    if total == 0 || done == 0 || done >= total {
        return None;
    }
    let secs = elapsed.as_secs_f64();
    // Half a second is long enough that the first number is not noise.
    if secs < 0.5 {
        return None;
    }
    let rate = done as f64 / secs;
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    let left = (total - done) as f64 / rate;
    Some((rate, std::time::Duration::from_secs_f64(left)))
}

/// The rate and ETA as one status-bar phrase, e.g. `12.3 MB/s · 4s left`.
/// Empty when there is nothing worth saying.
pub fn rate_eta_text(done: u64, total: u64, elapsed: std::time::Duration) -> String {
    let Some((rate, left)) = rate_and_eta(done, total, elapsed) else {
        return String::new();
    };
    let rate = crate::fs_model::fmt_size(rate as u64);
    let secs = left.as_secs();
    let left = if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        // Never "0s left" while there is still work: round up to one.
        format!("{}s", secs.max(1))
    };
    format!("{rate}/s \u{00B7} {left} left")
}

/// Total bytes under a path (file size, or recursive sum for folders).
pub fn dir_size(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
        if let Ok(md) = entry.metadata()
            && md.is_file()
        {
            total += md.len();
        }
    }
    total
}

/// What a folder adds up to, for the status bar and the details pane.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Measure {
    /// Total bytes of every file in the subtree.
    pub bytes: u64,
    /// Files in the subtree, at any depth.
    pub files: usize,
    /// Folders below the top one, at any depth.
    pub folders: usize,
}

impl Measure {
    /// One file, so a mixed selection can be totalled with folders.
    pub fn of_file(bytes: u64) -> Self {
        Self {
            bytes,
            files: 1,
            folders: 0,
        }
    }
}

/// Walks a folder and totals its subtree.
///
/// Symlinks are counted but never followed: a link pointing back up the tree
/// would otherwise walk forever.
pub fn measure(path: &Path) -> Measure {
    if path.is_file() {
        return Measure::of_file(path.metadata().map(|m| m.len()).unwrap_or(0));
    }
    let mut out = Measure::default();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().is_dir() {
            out.folders += 1;
        } else {
            out.files += 1;
            if let Ok(md) = entry.metadata() {
                out.bytes = out.bytes.saturating_add(md.len());
            }
        }
    }
    out
}

/// Subtree measurements for folders, filled in by a worker.
///
/// Measuring means walking, and a folder can hold hundreds of thousands of
/// entries. Doing that on the UI thread froze the window for seconds, so the
/// walk happens on a worker and the answer is kept until something changes it.
pub struct Measures {
    values: std::collections::HashMap<PathBuf, Measure>,
    asked: std::collections::HashSet<PathBuf>,
    tx: Sender<Msg>,
}

impl Measures {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self {
            values: std::collections::HashMap::new(),
            asked: std::collections::HashSet::new(),
            tx,
        }
    }

    /// The measurement for `path`, asking a worker for one if we lack it.
    ///
    /// Returns `None` until the answer lands, which is what lets the caller
    /// show a dash instead of blocking.
    pub fn get(&mut self, path: &Path) -> Option<Measure> {
        if let Some(m) = self.values.get(path) {
            return Some(*m);
        }
        if self.asked.insert(path.to_path_buf()) {
            let tx = self.tx.clone();
            let p = path.to_path_buf();
            // One thread per request, and `asked` keeps it to one per folder.
            let _ = std::thread::Builder::new()
                .name("rhumb-measure".into())
                .spawn(move || {
                    let _ = tx.send(Msg::Measured {
                        measure: measure(&p),
                        path: p,
                    });
                });
        }
        None
    }

    /// Records a worker's answer.
    pub fn set(&mut self, path: PathBuf, m: Measure) {
        self.asked.remove(&path);
        self.values.insert(path, m);
    }

    /// Forgets everything, so the next read is measured afresh.
    pub fn clear(&mut self) {
        self.values.clear();
        self.asked.clear();
    }
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
    paused: &AtomicBool,
) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(long_path(dest))?;
        for entry in walkdir::WalkDir::new(src).min_depth(1).sort_by_file_name() {
            wait_while_paused(paused, cancel);
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
                fs::create_dir_all(long_path(&target))?;
            } else {
                let name = rel.display().to_string();
                progress.current = name.clone();
                copy_file(entry.path(), &target, tx, progress, cancel, paused)?;
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
        copy_file(src, dest, tx, progress, cancel, paused)
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
        paused: Arc::new(AtomicBool::new(false)),
        started: std::time::Instant::now(),
    };
    let archive_name = archive_name_for(&sources);
    let dest = dest_dir.join(&archive_name);

    std::thread::Builder::new()
        .name("rhumb-zip".into())
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
                    let _ = fs::remove_file(long_path(&dest));
                    let _ = tx.send(Msg::Finished {
                        id,
                        outcome: Outcome::Failed(e),
                    });
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the zip worker: {e}"));
    job
}

/// Adds `sources` into an existing zip, on a thread of its own, the way Explorer
/// lets you paste into a compressed folder. The archive is rewritten beside
/// itself and only swapped in when the whole write has succeeded, so a failure
/// leaves the original alone.
pub fn start_add_to_zip(
    tx: Sender<Msg>,
    id: u64,
    archive: PathBuf,
    inner: String,
    sources: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
) -> Job {
    let name = short(&archive);
    let job = Job {
        id,
        kind: OpKind::Compress,
        label: format!("Adding to {name}"),
        cancel: cancel.clone(),
        paused: Arc::new(AtomicBool::new(false)),
        started: std::time::Instant::now(),
    };
    std::thread::Builder::new()
        .name("rhumb-zip-add".into())
        .spawn(move || {
            let total_items = sources.iter().map(|p| count_items(p)).sum::<usize>();
            let progress = Progress {
                id,
                done_items: 0,
                total_items,
                done_bytes: 0,
                total_bytes: 0,
                current: name,
                failed: Vec::new(),
            };
            let _ = tx.send(Msg::Progress(progress));
            // The rewrite itself cannot be stopped part-way, so a cancel is only
            // honoured before it begins.
            let outcome = if cancel.load(Ordering::Relaxed) {
                Outcome::Cancelled { done: 0 }
            } else {
                match crate::archive::add_to_zip(&archive, &inner, &sources) {
                    Ok(n) => Outcome::Done {
                        ok: n,
                        failed: Vec::new(),
                    },
                    Err(e) => Outcome::Failed(e),
                }
            };
            let _ = tx.send(Msg::Finished { id, outcome });
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the zip-add worker: {e}"));
    job
}

/// Removes `entries` (inner paths) from a zip, on a thread of its own. The
/// rewrite is written beside the archive and swapped in only when it is whole.
pub fn start_remove_from_zip(
    tx: Sender<Msg>,
    id: u64,
    archive: PathBuf,
    entries: Vec<String>,
    cancel: Arc<AtomicBool>,
) -> Job {
    let name = short(&archive);
    let count = entries.len();
    let job = Job {
        id,
        kind: OpKind::Delete,
        label: format!(
            "Removing {count} item{} from {name}",
            if count == 1 { "" } else { "s" }
        ),
        cancel: cancel.clone(),
        paused: Arc::new(AtomicBool::new(false)),
        started: std::time::Instant::now(),
    };
    std::thread::Builder::new()
        .name("rhumb-zip-del".into())
        .spawn(move || {
            let progress = Progress {
                id,
                done_items: 0,
                total_items: count,
                done_bytes: 0,
                total_bytes: 0,
                current: name,
                failed: Vec::new(),
            };
            let _ = tx.send(Msg::Progress(progress));
            // As with an add, a cancel is only honoured before the rewrite starts.
            let outcome = if cancel.load(Ordering::Relaxed) {
                Outcome::Cancelled { done: 0 }
            } else {
                match crate::archive::remove_from_zip(&archive, &entries) {
                    Ok(n) => Outcome::Done {
                        ok: n,
                        failed: Vec::new(),
                    },
                    Err(e) => Outcome::Failed(e),
                }
            };
            let _ = tx.send(Msg::Finished { id, outcome });
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the zip-del worker: {e}"));
    job
}

/// Extracts `inner` of an archive (everything, when it is empty) into `dest`, on a
/// thread of its own, reporting each file as it goes. The folder it writes to is made
/// if it is not there; if the job is cancelled, or fails, what was written is removed.
pub fn start_extract(
    tx: Sender<Msg>,
    id: u64,
    archive: PathBuf,
    inner: String,
    dest: PathBuf,
    cancel: Arc<AtomicBool>,
) -> Job {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let job = Job {
        id,
        kind: OpKind::Extract,
        label: format!("Extracting {name}"),
        cancel: cancel.clone(),
        paused: Arc::new(AtomicBool::new(false)),
        started: std::time::Instant::now(),
    };
    std::thread::Builder::new()
        .name("rhumb-extract".into())
        .spawn(move || {
            let (total_items, total_bytes) =
                crate::archive::count_files(&archive, &inner).unwrap_or((0, 0));
            let mut progress = Progress {
                id,
                done_items: 0,
                total_items,
                done_bytes: 0,
                total_bytes,
                current: name,
                failed: Vec::new(),
            };
            let _ = tx.send(Msg::Progress(progress.clone()));
            let made = !dest.exists();
            let result =
                crate::archive::extract_with(&archive, &inner, &dest, &mut |file, size| {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return false;
                    }
                    progress.done_items += 1;
                    progress.done_bytes += size;
                    progress.current = file.to_owned();
                    let _ = tx.send(Msg::Progress(progress.clone()));
                    true
                });
            let outcome = match result {
                Ok(done) => Outcome::Done {
                    ok: done.files,
                    failed: Vec::new(),
                },
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    // Half an extraction is worse than none: what this job made goes.
                    if made {
                        let _ = fs::remove_dir_all(long_path(&dest));
                    }
                    Outcome::Cancelled {
                        done: progress.done_items,
                    }
                }
                Err(e) => {
                    if made {
                        let _ = fs::remove_dir_all(long_path(&dest));
                    }
                    Outcome::Failed(e.to_string())
                }
            };
            let _ = tx.send(Msg::Finished { id, outcome });
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the extract worker: {e}"));
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

    let file = fs::File::create(long_path(dest)).map_err(|e| e.to_string())?;
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
            let mut r =
                fs::File::open(long_path(from)).map_err(|e| format!("{}: {e}", short(from)))?;
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
    paused: &AtomicBool,
) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(long_path(parent))?;
    }
    // Another process may hold either end for a moment; retry rather than fail.
    let mut r = retrying(|| fs::File::open(long_path(src)))?;
    let mut w = retrying(|| fs::File::create(long_path(dest)))?;
    let mut buf = vec![0u8; CHUNK];
    let mut last_report = std::time::Instant::now();
    loop {
        wait_while_paused(paused, cancel);
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
    // The destination can still be held by a scanner or indexer at this point.
    retrying(|| {
        let file = fs::File::options().write(true).open(long_path(dest))?;
        file.set_modified(md.modified()?)
    })
}

fn move_path(
    src: &Path,
    dest: &Path,
    tx: &Sender<Msg>,
    progress: &mut Progress,
    cancel: &AtomicBool,
    paused: &AtomicBool,
) -> io::Result<()> {
    // Fast path: same volume, no data copy needed. A rename can be refused
    // while something else has the file open, so it is retried; a genuine
    // cross-device failure falls through to copy + delete below.
    match retrying(|| fs::rename(long_path(src), long_path(dest))) {
        Ok(()) => {
            progress.done_bytes += dir_size(src);
            return Ok(());
        }
        Err(_) => { /* cross-device: fall through to copy + delete */ }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(long_path(parent))?;
    }
    copy_path(src, dest, tx, progress, cancel, paused)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    if src.is_dir() {
        fs::remove_dir_all(long_path(src))
    } else {
        fs::remove_file(long_path(src))
    }
}

/// Moves one item synchronously, for undo where the work must complete
/// before the next frame. Falls back to a real copy when the move crosses
/// volumes.
pub fn move_now(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(long_path(parent)).map_err(|e| e.to_string())?;
    }
    if fs::rename(long_path(src), long_path(dest)).is_ok() {
        return Ok(());
    }
    let (tx, rx) = crate::workers::bus();
    let cancel = AtomicBool::new(false);
    let paused = AtomicBool::new(false);
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
        fs::create_dir_all(long_path(dest)).map_err(|e| e.to_string())?;
        for entry in walkdir::WalkDir::new(src).min_depth(1).sort_by_file_name() {
            let entry = entry.map_err(|e| e.to_string())?;
            let rel = entry.path().strip_prefix(src).map_err(|e| e.to_string())?;
            let target = dest.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(long_path(&target)).map_err(|e| e.to_string())?;
            } else {
                copy_file(entry.path(), &target, &tx, &mut progress, &cancel, &paused)
                    .map_err(|e| e.to_string())?;
            }
        }
        fs::remove_dir_all(long_path(src)).map_err(|e| e.to_string())?;
    } else {
        copy_file(src, dest, &tx, &mut progress, &cancel, &paused).map_err(|e| e.to_string())?;
        fs::remove_file(long_path(src)).map_err(|e| e.to_string())?;
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
    let file = fs::File::open(long_path(path)).ok()?;
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
    let paused = Arc::new(AtomicBool::new(false));
    let job = Job {
        id,
        kind: OpKind::Delete,
        label: format!(
            "Deleting {} item{}",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ),
        cancel: cancel.clone(),
        paused: paused.clone(),
        started: std::time::Instant::now(),
    };
    let spawned = Job {
        id,
        kind: job.kind,
        label: job.label.clone(),
        cancel: cancel.clone(),
        paused: paused.clone(),
        started: job.started,
    };
    std::thread::Builder::new()
        .name("rhumb-del".into())
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
                    fs::remove_dir_all(long_path(p))
                } else {
                    fs::remove_file(long_path(p))
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
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the delete worker: {e}"));
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::bus;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rhumb-op-{name}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn measure_totals_the_whole_subtree() {
        let root = tmp("measure");
        let a = root.join("a");
        fs::create_dir_all(a.join("deep/deeper")).unwrap();
        fs::write(root.join("top.txt"), "12345").unwrap();
        fs::write(a.join("one.txt"), "123").unwrap();
        fs::write(a.join("deep/two.txt"), "1234").unwrap();
        fs::write(a.join("deep/deeper/three.txt"), "12").unwrap();

        let m = measure(&root);
        assert_eq!(m.files, 4, "should count files at every depth");
        // a, a/deep, a/deep/deeper: everything below the top.
        assert_eq!(m.folders, 3, "should count folders at every depth");
        assert_eq!(m.bytes, 5 + 3 + 4 + 2);
    }

    #[test]
    fn measure_of_a_file_is_just_itself() {
        let root = tmp("measure-file");
        let f = root.join("solo.txt");
        fs::write(&f, "abcd").unwrap();
        assert_eq!(measure(&f), Measure::of_file(4));
    }

    #[test]
    fn measure_does_not_follow_a_link_back_up_the_tree() {
        let root = tmp("measure-loop");
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("f.txt"), "x").unwrap();
        // A link pointing at an ancestor: following it would never terminate.
        let link = sub.join("up");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&root, &link).is_ok() {
            let m = measure(&sub);
            // The file, and the link itself counted as an entry; what it points at is
            // not walked, so no folder is found and nothing from `root` is added.
            assert_eq!(m.files, 2, "the link's contents were walked");
            assert_eq!(m.folders, 0, "the link's contents were walked");
            return;
        }
        let m = measure(&sub);
        assert_eq!(m.files, 1);
        assert_eq!(m.bytes, 1);
    }

    #[test]
    fn measures_asks_once_and_then_answers_from_cache() {
        let root = tmp("measure-cache");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/f.txt"), "xy").unwrap();
        let dir = root.join("sub");

        let (tx, rx) = bus();
        let mut cache = Measures::new(tx);

        // Nothing known yet, and one request goes out.
        assert_eq!(cache.get(&dir), None);
        assert_eq!(cache.get(&dir), None, "asked twice for the same folder");
        assert!(rx.try_recv().is_err(), "the ask is not a message");

        // The worker answers, and after that it is a plain lookup.
        cache.set(dir.clone(), measure(&dir));
        assert_eq!(cache.get(&dir), Some(Measure::of_file(2)));

        // Clearing makes it ask again, because the answer may be stale.
        cache.clear();
        assert_eq!(cache.get(&dir), None);
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
    fn copy_and_move_reach_past_max_path() {
        if !cfg!(windows) {
            return;
        }
        let root = tmp("long-path");
        let deep = deeper_than(&root, 300);
        assert!(deep.as_os_str().len() > 260, "the tree is not long enough");
        fs::create_dir_all(fs_model::long_path(&deep)).unwrap();
        let src = deep.join("a.txt");
        fs::write(fs_model::long_path(&src), b"hello").unwrap();

        // Copy the deep file out to the short root: this is `copy_file`, which
        // opens and creates both ends through the helper.
        let copied = root.join("copy.txt");
        let (tx, _rx) = bus();
        let cancel = AtomicBool::new(false);
        let paused = AtomicBool::new(false);
        let mut progress = Progress {
            id: 1,
            done_items: 0,
            total_items: 1,
            done_bytes: 0,
            total_bytes: 5,
            current: String::new(),
            failed: Vec::new(),
        };
        copy_file(&src, &copied, &tx, &mut progress, &cancel, &paused).unwrap();
        assert_eq!(fs::read(&copied).unwrap(), b"hello");

        // Move it back down, which exercises `rename` on a verbatim path.
        let moved = deep.join("moved.txt");
        move_now(&copied, &moved).unwrap();
        assert_eq!(fs::read(fs_model::long_path(&moved)).unwrap(), b"hello");
        assert!(!copied.exists());

        let _ = fs::remove_dir_all(fs_model::long_path(&root));
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

    #[test]
    fn retrying_retries_a_sharing_violation_then_succeeds() {
        let mut calls = 0;
        let r: io::Result<u32> = retrying(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(7)
            }
        });
        assert_eq!(r.unwrap(), 7);
        assert_eq!(calls, 3, "should have tried again until it worked");
    }

    #[test]
    fn retrying_gives_up_after_its_attempts() {
        let mut calls = 0;
        let r: io::Result<()> = retrying(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(33))
        });
        assert!(r.is_err());
        assert_eq!(calls, RETRY_ATTEMPTS, "gave up before the limit");
    }

    #[test]
    fn retrying_does_not_retry_an_unrelated_error() {
        let mut calls = 0;
        let r: io::Result<()> = retrying(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(2))
        });
        assert!(r.is_err());
        assert_eq!(calls, 1, "a missing file is not worth trying again");
    }

    #[test]
    fn rate_and_eta_reports_a_sane_rate_and_time() {
        // 1 MiB in 1 s means 1 MiB/s and, with 2 MiB to go, 2 s left.
        let (rate, eta) = rate_and_eta(
            1024 * 1024,
            3 * 1024 * 1024,
            std::time::Duration::from_secs(1),
        )
        .expect("a rate once there is a total and some time");
        assert!((rate - 1024.0 * 1024.0).abs() < 1.0, "rate was {rate}");
        assert_eq!(eta.as_secs(), 2);
    }

    #[test]
    fn rate_and_eta_is_none_when_there_is_nothing_to_go_on() {
        let a_second = std::time::Duration::from_secs(1);
        assert!(rate_and_eta(0, 100, a_second).is_none(), "no progress yet");
        assert!(rate_and_eta(50, 0, a_second).is_none(), "unknown total");
        assert!(rate_and_eta(100, 100, a_second).is_none(), "already there");
        assert!(
            rate_and_eta(50, 100, std::time::Duration::from_millis(10)).is_none(),
            "too soon to guess"
        );
    }

    #[test]
    fn rate_eta_text_reads_like_a_status_line() {
        let text = rate_eta_text(
            1024 * 1024,
            3 * 1024 * 1024,
            std::time::Duration::from_secs(1),
        );
        assert!(text.contains("/s"), "{text}");
        assert!(text.contains("left"), "{text}");
        assert_eq!(rate_eta_text(0, 0, std::time::Duration::from_secs(1)), "");
    }

    #[test]
    fn a_paused_copy_waits_until_it_is_resumed() {
        let root = tmp("pause");
        let src = root.join("src.bin");
        let data = vec![7u8; 256 * 1024];
        fs::write(&src, &data).unwrap();
        let dest = root.join("dest.bin");

        let (tx, _rx) = bus();
        let cancel = Arc::new(AtomicBool::new(false));
        // Held from the very first chunk, so the copy cannot run ahead of the
        // check below however the threads are scheduled.
        let paused = Arc::new(AtomicBool::new(true));
        let mut progress = Progress {
            id: 1,
            done_items: 0,
            total_items: 1,
            done_bytes: 0,
            total_bytes: data.len() as u64,
            current: String::new(),
            failed: Vec::new(),
        };

        let (p, c) = (paused.clone(), cancel.clone());
        let (s, d) = (src.clone(), dest.clone());
        let worker = std::thread::spawn(move || copy_file(&s, &d, &tx, &mut progress, &c, &p));

        std::thread::sleep(std::time::Duration::from_millis(120));
        assert_eq!(
            fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
            0,
            "bytes were written while the job was paused"
        );

        paused.store(false, Ordering::Relaxed);
        worker.join().unwrap().unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data, "the copy never finished");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancel_releases_a_paused_worker() {
        let paused = Arc::new(AtomicBool::new(true));
        let cancel = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (p, c) = (paused.clone(), cancel.clone());
        let worker = std::thread::spawn(move || {
            wait_while_paused(&p, &c);
            done_tx.send(()).unwrap();
        });
        // A pause must not make a cancel unreachable.
        cancel.store(true, Ordering::Relaxed);
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok(),
            "a cancelled worker stayed stuck behind the pause"
        );
        worker.join().unwrap();
    }
}
