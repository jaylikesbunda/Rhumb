# Rhumb

A dark, fast file explorer with a built-in code editor and live Markdown
preview. Native Rust and [egui](https://github.com/emilk/egui): one binary, no
runtime.

## Features

**Browsing**
- Folder tabs, each with its own place, history, selection and open files
- Details, compact list and large-icon views with image thumbnails
- Breadcrumb path bar, sortable resizable columns, quick access with pinning
- Search as you type: this folder, or everything below it, cancellable
- Name index per folder for instant ranked search, kept current by a watcher
- ZIP, TAR and TAR.GZ archives open like folders (read-only); extract from the
  context menu
- Copy, cut, paste, rename, trash, delete, zip, properties, undo, drag and drop
- Native file clipboard, so files move to and from Explorer

**Editing**
- Rope-backed editor that stays responsive on files with 100,000+ lines
- Multi-cursor: `Alt+Click`, `Ctrl+D` (next occurrence), `Ctrl+Shift+L`
  (all occurrences), column selection, one undo step per multi-edit
- Syntax highlighting, bracket matching, auto-close, comment toggling, soft
  wrap, find and replace, focus mode
- Live Markdown preview beside the source, with optional scroll lock

**Interface**
- Settings window, zoom slider, three views, light touch of chrome
- Phosphor icon set

## Performance

Frame costs measured on the release build with 20,000 to 100,000 line files,
including layout and tessellation (GPU time not included), on one Windows 11 desktop under normal load. Medians, in milliseconds. A frame at 240 fps
is 4.2 ms.

| Operation | Median | Worst |
|:--|--:|--:|
| Typed character, whole app, 20,000 lines | 0.74 | 2.0 |
| Idle frame, whole app | 0.53 | 1.2 |
| Scrolling the editor, whole app | 0.53 | 1.2 |
| Arrow key in a 100,000-line file | 0.29–0.38 | 1.3 |
| Page Down / Ctrl+End in 100,000 lines | 0.46 / 0.55 | 1.2 |
| Enter / Backspace mid-file, 20,000 lines | 0.22 / 0.19 | 0.8 |
| Undo / redo after a long session | 0.53 / 0.51 | 3.1 |
| Frame over a 100 KB single line | 0.01 | 0.05 |
| Typing into a 100 KB single line | 0.55 | 1.3 |
| Paste 100 KB, then undo it | 3.7 / 3.3 | |

The release binary is about 16 MB. Reproduce with:

    cargo test --release fits_in_a_frame -- --nocapture --test-threads=1
    cargo test --release instant -- --nocapture --test-threads=1

These are in-process frame timings, not a comparison against other programs.
Set `RHUMB_STRICT_SPEED=1` to hold the tests to the 240 fps budget on a quiet
machine.

## Where it is stronger

- **One tool for browsing and editing.** Open a file from the list and edit it
  in place. Markdown gets a live preview in the same tab.
- **Cost per keystroke does not grow with the file.** Edits and scrolling work
  on a rope and only touch the visible lines.
- **Searching never blocks the window.** Folders are read, indexed and searched
  on worker threads.
- **Small and self-contained.** One executable, no installer needed, no
  telemetry.

## Limits

- Windows is the primary platform. Clipboard integration and single-window
  handling are Windows-specific; Linux builds compile but get less testing.
- No language server, tree-sitter, code folding or IME composition in the
  editor.
- Archives are read-only (extract to change them). 7z and RAR are not
  supported.
- No dual-pane view, no Recycle Bin browser, no shell registration as the
  default folder handler.
- The name index is in memory and rebuilt per session.

## Install

Download a build from the releases page:

| File | |
|:--|:--|
| `rhumb-<version>-x64.msi` | Windows installer |
| `rhumb-<version>-windows-portable.zip` | Windows, no install |

Settings are stored in `%LOCALAPPDATA%\rhumb\prefs.txt`. Delete it to reset.

## Build

Needs Rust 1.85 or newer.

    cargo build --release
    cargo test
    cargo clippy --all-targets

`tools/build_msi.sh` builds the installer (needs WiX v3 on `PATH`).

## Releases

Run the **release** workflow from the Actions tab and enter a version such as
`0.2.0`. It stamps that version into the build, produces the MSI and portable
zip, tags `v<version>` and publishes the release.

## Keys

| Shortcut | Action |
|:---------|:-------|
| `Ctrl+1` / `2` / `3` | Details / list / large-icon view |
| `Ctrl+B`, `Alt+P` | Toggle sidebar / details pane |
| `Alt+Left` / `Right` / `Up` | Back, forward, up |
| `Ctrl+L`, `Ctrl+F` | Type a path / focus search |
| `Ctrl+H` | Show hidden items |
| `Ctrl+N` / `Ctrl+Shift+N` | New text document / folder |
| `Ctrl+T` | New folder tab |
| `Ctrl+Tab`, `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+W` | Close the file, or the folder tab if none is open |
| `Ctrl+A`, `C`, `X`, `V` | Selection and clipboard |
| `F2`, `Delete`, `Shift+Delete` | Rename, trash, delete permanently |
| `Ctrl+Z` | Undo the last file operation |
| `Alt+Enter` | Properties |
| `Ctrl+S` | Save |
| `Alt+Click`, `Ctrl+D`, `Ctrl+Shift+L` | Multi-cursor |
| `Tab` / `Shift+Tab`, `Ctrl+/` | Indent, toggle comment |
| `F5` | Reload the folder |
| `/` or `?` | Shortcut help |

## Licence

MIT. See [LICENSE](LICENSE).
