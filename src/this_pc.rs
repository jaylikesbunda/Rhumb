//! "This PC" as a place the file list can show.
//!
//! Like the Recycle Bin, This PC names no folder on any disk: it is a view that
//! gathers the drives and the user folders into one listing. [`is_root`]
//! recognises the one magic path the sidebar opens, and [`list`] builds the
//! rows from the same drives and folders the sidebar already knows, so the two
//! agree on what a place is called.
//!
//! The one thing that needs the OS is a mapped network drive: a letter whose
//! real home is a share should read as the share, not as a bare letter. That
//! lookup is Windows-only and falls back to the drive's own label elsewhere.
//!
//! A drive plugged in or removed is picked up by the sidebar's `Roots` worker,
//! which re-reads the volumes every few seconds; a listing built from it is
//! therefore never more than one refresh stale. No window message hook is
//! installed for that, deliberately.

use std::path::{Path, PathBuf};

use crate::fs_model::{self, Entry};

/// The magic path the sidebar's This PC row points at. It names no folder on
/// any disk, which is what keeps it from colliding with a real one.
pub const ROOT: &str = "::ThisPC::";

/// Whether this is the magic This PC path.
pub fn is_root(path: &Path) -> bool {
    path.as_os_str() == std::ffi::OsStr::new(ROOT)
}

/// The user folders and the drives, as one listing.
///
/// Every row is a folder, because each one is somewhere to walk into. Free
/// space is not carried here: the sidebar already shows it for the same drive
/// paths from its own cache, and the file list has no column for it.
pub fn list() -> Vec<Entry> {
    let mut out = Vec::new();
    for place in fs_model::user_folders() {
        out.push(row(place.label, place.path));
    }
    for drive in fs_model::drives() {
        // A mapped drive reads as the share it points at; an ordinary volume
        // keeps the label the sidebar built, which carries the letter.
        let name = unc_target(&drive.path).unwrap_or(drive.label);
        out.push(row(name, drive.path));
    }
    out
}

/// One row of This PC: a folder that can be walked into.
fn row(name: String, path: PathBuf) -> Entry {
    Entry {
        name,
        path,
        is_dir: true,
        is_symlink: false,
        size: 0,
        modified: None,
        hidden: false,
    }
}

/// The UNC path a mapped network drive points at, e.g. `\\server\share`.
///
/// A mapped drive keeps its letter, but its real home is a share and the letter
/// alone says nothing about where that is. `WNetGetConnectionW` answers for a
/// letter that is mapped and fails for a local volume; anything that is not a
/// bare drive letter has no mapping to ask about.
#[cfg(windows)]
pub fn unc_target(path: &Path) -> Option<String> {
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;

    let letter = drive_letter(path)?;
    // `WNetGetConnectionW` names a local device as "Z:", NUL-terminated.
    let local: Vec<u16> = letter.encode_utf16().chain(std::iter::once(0)).collect();
    // A share name is short; 512 UTF-16 units is more than any real mapping, and
    // one longer than that falls back to the label rather than needing a second
    // call just to size the buffer.
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    // SAFETY: `local` is a NUL-terminated UTF-16 string alive for the call, and
    // `buf` is `len` writable UTF-16 units; the API writes no more than that
    // and reports how many it used.
    let rc = unsafe { WNetGetConnectionW(local.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if rc != NO_ERROR {
        // A local volume answers ERROR_NOT_CONNECTED; every other code means
        // this letter does not name a share either.
        return None;
    }
    // `len` counts the terminating NUL, so the text stops one short of it.
    let text = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
    let text = text.trim_end_matches('\0');
    // Only a UNC path is a mapping; anything else is not this.
    (text.starts_with(r"\\") && text.len() > 2).then(|| text.to_owned())
}

/// There are no drive letters off Windows, so there is no mapping to resolve.
#[cfg(not(windows))]
pub fn unc_target(_path: &Path) -> Option<String> {
    None
}

/// The `Z:` form of a Windows drive root, or `None` when the path is not one.
#[cfg(windows)]
fn drive_letter(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    let s = s.trim_end_matches(['\\', '/']);
    let bytes = s.as_bytes();
    (bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':').then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_root_only_matches_the_magic_path() {
        assert!(is_root(Path::new(ROOT)));
        assert!(!is_root(Path::new(r"C:\")));
        assert!(!is_root(Path::new(r"::ThisPC::\x")));
        assert!(!is_root(Path::new("")));
    }

    #[test]
    fn list_holds_every_user_folder_and_every_drive() {
        let entries = list();
        // Both the folders and the drives are rows to walk into.
        assert!(entries.iter().all(|e| e.is_dir), "{entries:?}");
        for place in fs_model::user_folders() {
            assert!(
                entries.iter().any(|e| e.path == place.path),
                "{} is missing from This PC",
                place.label
            );
        }
        for drive in fs_model::drives() {
            assert!(
                entries.iter().any(|e| e.path == drive.path),
                "{} is missing from This PC",
                drive.path.display()
            );
        }
        // This PC does not contain itself, and the Recycle Bin is a place of
        // its own rather than a folder in here.
        assert!(!entries.iter().any(|e| e.path == Path::new(ROOT)));
        #[cfg(windows)]
        assert!(
            !entries
                .iter()
                .any(|e| e.path == Path::new(crate::recycle::ROOT))
        );
    }

    #[test]
    fn list_has_the_home_folder_and_the_drives_this_machine_has() {
        let entries = list();
        if dirs::home_dir().is_some() {
            assert!(entries.iter().any(|e| e.name == "Home"), "{entries:?}");
        }
        // A Windows machine has at least one drive letter, and it must be in
        // the listing rather than only in the sidebar.
        #[cfg(windows)]
        assert!(
            entries.iter().any(|e| {
                let s = e.path.to_string_lossy();
                s.len() == 3 && s.ends_with(":\\") && s.as_bytes()[0].is_ascii_alphabetic()
            }),
            "no drive letter in This PC: {entries:?}"
        );
    }

    #[test]
    fn a_local_drive_has_no_unc_target() {
        // A drive letter that is not a mapping answers with nothing rather than
        // guessing; the caller then keeps the label.
        #[cfg(windows)]
        {
            assert!(unc_target(Path::new(r"C:\")).is_none());
            assert!(unc_target(Path::new(r"C:")).is_none());
        }
        // A path with no letter to look up is not a mapping anywhere.
        assert!(unc_target(Path::new("/home/dev")).is_none());
        assert!(unc_target(Path::new(r"\\server\share")).is_none());
    }
}
