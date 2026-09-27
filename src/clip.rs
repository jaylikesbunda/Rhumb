//! Native file clipboard, so copy and paste work across apps.
//!
//! arboard only speaks text, which is why copying here and pasting in
//! Explorer did nothing (Explorer wants CF_HDROP), and pasting Explorer's
//! files here failed the same way in reverse. This module speaks the two
//! formats Explorer cares about — CF_HDROP and "Preferred DropEffect" —
//! through raw Win32 calls, no COM involved.
//!
//! Any clipboard call can fail (another app holding it open is the usual
//! cause), so everything returns `Option` and callers fall back to the text
//! clipboard or a toast.

/// Files on the system clipboard and whether they were cut.
pub struct Hdrop {
    pub paths: Vec<std::path::PathBuf>,
    /// True when the source marked these as a cut (move on paste).
    pub cut: bool,
}

/// Reads Explorer-style files off the clipboard, if any are there.
pub fn read_hdrop() -> Option<Hdrop> {
    imp::read()
}

/// Offers files to other apps, marking them cut or copied.
pub fn write_hdrop(paths: &[std::path::PathBuf], cut: bool) {
    imp::write(paths, cut);
}

#[cfg(windows)]
mod imp {
    use super::Hdrop;
    use std::path::PathBuf;

    /// What Explorer does with dropped files. Copy is 1, move is 2.
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;

    fn drop_effect_format() -> Option<std::num::NonZeroU32> {
        clipboard_win::register_format("Preferred DropEffect")
    }

    /// First DWORD of the effect blob, little-endian, as Windows stores it.
    fn effect_of(bytes: &[u8]) -> Option<u32> {
        bytes
            .first_chunk()
            .map(|head: &[u8; 4]| u32::from_le_bytes(*head))
    }

    pub fn read() -> Option<Hdrop> {
        use clipboard_win::{formats::FileList, get_clipboard, is_format_avail};
        if !is_format_avail(clipboard_win::formats::CF_HDROP) {
            return None;
        }
        let paths: Vec<PathBuf> = get_clipboard(FileList).ok()?;
        if paths.is_empty() || !paths.iter().any(|p| p.exists()) {
            return None;
        }
        // No effect recorded means a copy: only an explicit move cuts.
        let cut = drop_effect_format()
            .and_then(|f| get_clipboard(clipboard_win::formats::RawData(f.get())).ok())
            .and_then(|bytes: Vec<u8>| effect_of(&bytes))
            == Some(DROPEFFECT_MOVE);
        Some(Hdrop { paths, cut })
    }

    pub fn write(paths: &[PathBuf], cut: bool) {
        let texts: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        // The guard holds the clipboard open; dropping it closes it again.
        let Ok(_open) = clipboard_win::Clipboard::new() else {
            return;
        };
        // This clears first, so it goes before the effect below.
        if clipboard_win::raw::set_file_list(&texts).is_err() {
            return;
        }
        let effect = if cut {
            DROPEFFECT_MOVE
        } else {
            DROPEFFECT_COPY
        };
        if let Some(format) = drop_effect_format() {
            let _ = clipboard_win::raw::set_without_clear(format.get(), &effect.to_le_bytes());
        }
    }

    #[cfg(test)]
    mod tests {
        use super::effect_of;

        #[test]
        fn drop_effect_reads_the_first_dword() {
            assert_eq!(effect_of(&[2, 0, 0, 0]), Some(2));
            assert_eq!(effect_of(&[1, 0, 0, 0]), Some(1));
            // Whatever trails the DWORD is someone else's business.
            assert_eq!(effect_of(&[2, 0, 0, 0, 9, 9]), Some(2));
        }

        #[test]
        fn drop_effect_needs_four_bytes() {
            assert_eq!(effect_of(&[]), None);
            assert_eq!(effect_of(&[2, 0, 0]), None);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Hdrop;
    use std::path::PathBuf;

    pub fn read() -> Option<Hdrop> {
        None
    }

    pub fn write(_paths: &[PathBuf], _cut: bool) {}
}
