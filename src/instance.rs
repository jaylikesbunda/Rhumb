//! Keeps one window per user: a second launch hands its path to the first.
//!
//! Two windows meant two independent histories, two search boxes, and two
//! places for a folder to be. The mechanism is a named mutex the first
//! window owns, plus a signal file a second launch writes for it to pick up.
//! No COM, and the same shape works on Linux through a lock file, so the
//! rules can be tested here rather than only on Windows.
//!
//! The cost is that the first window has to look at the signal file now and
//! then, which a small thread does, waking the window only when there is
//! something to read, so an idle window is not redrawn to check.

use std::path::{Path, PathBuf};

/// The signal file, kept at one fixed per-user path so the owner and the guest
/// cannot disagree about where it is.
fn signal_file() -> Option<PathBuf> {
    let dir = dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("rhumb");
    Some(dir.join("instance.signal"))
}

/// Ownership of the instance slot, held for as long as this value lives.
///
/// On Windows that is a named mutex the process keeps open; elsewhere it is a
/// lock file holding this process's pid. A crash leaves the file behind, but
/// the next launch sees the dead pid and takes it over, so a crash never leaves
/// the app unlaunchable.
pub struct Lock {
    /// Held only so the handle stays open: the lock belongs to the process,
    /// not to this value. Named `_` so nothing reads it by accident.
    #[cfg(windows)]
    _handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    _file: std::fs::File,
    _dir: PathBuf,
}

/// Takes the lock, if it is free. `None` means another instance has it.
pub fn try_acquire() -> Option<Lock> {
    platform::try_acquire()
}

/// Claims the slot for this window, asking any window that already has it to
/// open `path` instead of opening a second one.
///
/// Returns `true` when the caller should carry on starting up.
pub fn claim(path: Option<&Path>) -> bool {
    match try_acquire() {
        Some(lock) => {
            // A stale request from a crash must not be read as a fresh one.
            // The wait covers the other race: a guest writing right now, whose
            // rename would otherwise land after we cleared the file.
            if let Some(sig) = signal_file() {
                let _ = std::fs::remove_file(&sig);
                if !wait_until_released(&sig, std::time::Duration::from_millis(500)) {
                    log::warn!("instance signal still present; a request may be lost");
                }
            }
            keep(lock);
            true
        }
        None => {
            signal(path);
            false
        }
    }
}

/// Keeps the lock alive for the rest of the process by leaking it.
fn keep(lock: Lock) {
    std::mem::forget(lock);
}

/// Asks the window that holds the lock to open `path`, or just come forward.
pub fn signal(path: Option<&Path>) {
    let Some(sig) = signal_file() else {
        return;
    };
    signal_in(&sig, path);
}

/// The signal write, against an explicit path so the rules can be tested
/// without touching the real one.
fn signal_in(signal: &Path, path: Option<&Path>) {
    if let Some(dir) = signal.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = path
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Written whole or not at all: the owner moves it into place with a
    // rename, so it never reads a half-written path.
    let staging = signal.with_extension("signal.new");
    if std::fs::write(&staging, body).is_ok() {
        let _ = std::fs::rename(&staging, signal);
    }
}

/// Watches for a signal from a second launch, and wakes the window for it.
///
/// Looking at a file is nothing; drawing a frame is not, so the looking is done
/// here and a frame is asked for only when there is something to act on.
pub fn watch(ctx: egui::Context) {
    let Some(sig) = signal_file() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("instance-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if sig.exists() {
                    ctx.request_repaint();
                    // Not asked again until the frame has taken it.
                    while sig.exists() {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        ctx.request_repaint();
                    }
                }
            }
        });
}

/// Takes the signal if one is waiting, so it is acted on exactly once.
///
/// `Some(Some(path))` opens the path, `Some(None)` just raises the window,
/// and `None` means nothing was waiting.
pub fn take_signal() -> Option<Option<PathBuf>> {
    take_signal_in(&signal_file()?)
}

/// The signal read, against an explicit path.
fn take_signal_in(signal: &Path) -> Option<Option<PathBuf>> {
    // Renaming first consumes it, so two frames cannot both act on it.
    let staging = signal.with_extension("signal.reading");
    match std::fs::rename(signal, &staging) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("cannot read the instance signal: {e}");
            return None;
        }
    }
    let body = std::fs::read_to_string(&staging).unwrap_or_default();
    let _ = std::fs::remove_file(&staging);
    let body = body.trim();
    Some((!body.is_empty()).then(|| PathBuf::from(body)))
}

/// Waits for `signal` to be gone, up to `limit`, so the owner does not read
/// its own removal as a live request from another window.
fn wait_until_released(signal: &Path, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if !signal.exists() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(windows)]
mod platform {
    use super::Lock;
    use std::path::PathBuf;

    /// Must match whatever the first window created.
    const MUTEX: &str = r"Local\rhumb-single-instance";

    /// NUL-terminated UTF-16, as every Win32 wide call wants.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn try_acquire() -> Option<Lock> {
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE,
        };
        use windows_sys::Win32::System::Threading::CreateMutexW;
        unsafe {
            // bInitialOwner = 1: we own it, so nobody else can.
            let handle: HANDLE = CreateMutexW(std::ptr::null(), 1, wide(MUTEX).as_ptr());
            if handle.is_null() {
                // Cannot even create the mutex. A second window beats no
                // window, so carry on without the lock.
                log::warn!("cannot create the instance mutex; another window may open");
                return None;
            }
            // Read the error before anything else can overwrite it.
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // Someone else got there first. Our handle is a second
                // reference to theirs, so drop it and stand down.
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Lock {
                _handle: handle,
                _dir: PathBuf::from(MUTEX),
            })
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::Lock;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// Where the lock lives: the session's runtime dir when it has one, else a
    /// per-user folder in the temp dir. `XDG_RUNTIME_DIR` is per-user and is
    /// cleared at logout, which is exactly the lifetime wanted; the temp dir is
    /// the fallback, keyed by uid so two users on one machine do not collide.
    fn lock_path() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            return Some(dir.join("rhumb.lock"));
        }
        Some(
            std::env::temp_dir()
                .join(format!("rhumb-{}", user_id()))
                .join("rhumb.lock"),
        )
    }

    /// This process's real user id. No crate for one syscall.
    fn user_id() -> u32 {
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        // SAFETY: `getuid` takes no arguments and cannot fail.
        unsafe { getuid() }
    }

    /// Whether `pid` still names a live process. `kill(pid, 0)` sends no signal;
    /// it only asks the kernel. `EPERM` means alive but owned by someone else,
    /// which still counts as alive.
    fn is_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const EPERM: i32 = 1;
        // SAFETY: signal 0 is an existence probe and cannot affect `pid`.
        if unsafe { kill(pid as i32, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
    }

    /// Creates the lock, failing if it already exists, and records our pid.
    /// `create_new` is `O_EXCL`, so two launches cannot both succeed.
    fn create(path: &Path) -> std::io::Result<std::fs::File> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        // The pid is what a later launch reads to tell a live owner from a
        // crash's leftovers.
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();
        Ok(file)
    }

    pub fn try_acquire() -> Option<Lock> {
        let path = lock_path()?;
        let dir = path.parent()?.to_path_buf();
        std::fs::create_dir_all(&dir).ok()?;

        match create(&path) {
            Ok(file) => {
                return Some(Lock {
                    _file: file,
                    _dir: dir,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                log::warn!("cannot create the instance lock: {e}");
                return None;
            }
        }

        // The file is there. A live owner keeps the slot; a dead pid is a crash
        // we are free to take over.
        let owner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        if owner.is_some_and(is_alive) {
            return None;
        }
        // Stale: clear it and claim it. `create_new` decides a race between two
        // launches taking over the same dead owner.
        let _ = std::fs::remove_file(&path);
        match create(&path) {
            Ok(file) => Some(Lock {
                _file: file,
                _dir: dir,
            }),
            Err(_) => None,
        }
    }
}

#[cfg(all(not(windows), not(unix)))]
mod platform {
    use super::Lock;
    use std::path::PathBuf;

    pub fn try_acquire() -> Option<Lock> {
        Some(Lock {
            _dir: PathBuf::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private directory per test, so a test neither sees nor disturbs a
    /// real running instance.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rhumb-instance-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_signal_is_seen_once_and_then_gone() {
        let sig = scratch("signal-once").join("instance.signal");
        std::fs::write(&sig, r"C:\work\notes.md").unwrap();
        let got = take_signal_in(&sig).expect("a signal was waiting");
        assert_eq!(got, Some(PathBuf::from(r"C:\work\notes.md")));
        assert!(
            take_signal_in(&sig).is_none(),
            "the signal was acted on twice"
        );
    }

    #[test]
    fn a_signal_with_no_path_means_just_raise_the_window() {
        let sig = scratch("signal-focus").join("instance.signal");
        std::fs::write(&sig, "").unwrap();
        assert_eq!(take_signal_in(&sig), Some(None), "empty means focus only");
    }

    #[test]
    fn a_half_written_path_is_never_read() {
        // `signal_in` writes through a staging file, so what the owner sees is
        // always a whole path or nothing.
        let sig = scratch("signal-atomic").join("instance.signal");
        signal_in(&sig, Some(Path::new(r"C:\work\notes.md")));
        assert!(
            !sig.with_extension("signal.new").exists(),
            "staging was left behind"
        );
        assert_eq!(
            take_signal_in(&sig),
            Some(Some(PathBuf::from(r"C:\work\notes.md")))
        );
    }

    #[test]
    fn waiting_for_release_gives_up_rather_than_hanging() {
        let sig = scratch("release").join("instance.signal");
        std::fs::write(&sig, "stale").unwrap();
        assert!(!wait_until_released(
            &sig,
            std::time::Duration::from_millis(60)
        ));
        std::fs::remove_file(&sig).unwrap();
        assert!(wait_until_released(
            &sig,
            std::time::Duration::from_millis(60)
        ));
    }
}
