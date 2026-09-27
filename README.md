# Xplor

A minimal, fast, dark file explorer with a built-in text editor and a live
Markdown preview. Native Rust and [egui](https://github.com/emilk/egui), one
binary, no runtime to install.

<p align="center">
  <img src="assets/icon.png" width="96" alt="Xplor icon">
</p>

## What it does

- **Browse** with a lazy, Explorer-style tree in the sidebar, an address bar of
  clickable path segments, and a sortable list.
- **Tabs**, like Windows 11 Explorer: `Ctrl+T` opens a new tab on the file
  list, opening a file turns that tab into the document, and `Ctrl+Tab` moves
  between them. Unsaved tabs show a dot and ask before closing.
- **Three views** — details with sortable, drag-to-resize columns, a compact
  list, and large icons with real image thumbnails. `Ctrl+1`, `Ctrl+2`,
  `Ctrl+3`, or the switch in the bottom-right corner. Columns give way to the
  name in a narrow pane, and come back when there is room.
- **Search as you type**, in this folder or in everything below it, with the
  search cancellable and never blocking the window.
- **Edit text and code** with syntax highlighting, bracket matching, auto-close,
  comment toggling and soft wrap.
- **Preview Markdown** side by side with the source: headings, lists, task
  lists, tables, quotes, code blocks, links, inline code and images.
- **A details pane** with the file's facts and a preview of its contents: the
  thumbnail for an image, the first lines for a text file.
- **File operations** with real progress and a cancel button: copy, cut, paste,
  rename, delete to the recycle bin or permanently, compress to ZIP, copy a
  path, open in a terminal, and undo with `Ctrl+Z`.
- **Drag and drop**, both files from the desktop and rows onto folders, to move
  or (with `Ctrl`) copy.

## Install

Grab a build from the releases page:

| Platform | File |
|:---------|:-----|
| Windows  | `xplor-<version>-x64.msi` |
| Linux    | `xplor-<version>-x86_64.AppImage` |

The MSI is a normal installer with a Start-menu shortcut. The AppImage is
self-contained:

```sh
chmod +x xplor-*.AppImage
./xplor-*.AppImage
```

## Build from source

Needs a Rust toolchain (1.85 or newer).

```sh
git clone https://github.com/you/xplor
cd xplor
cargo build --release      # target/release/xplor
cargo test                 # 71 unit tests
cargo clippy --all-targets # lints
```

The release profile is tuned for size and speed: `opt-level=3`, thin LTO, one
codegen unit, `panic=abort` and stripped symbols, which lands around 15 MB.

## Keyboard

| Shortcut | Action |
|:---------|:-------|
| `Ctrl+1` / `2` / `3` | Details / list / large-icon view |
| `Ctrl+Shift+V` | Cycle the view |
| `Ctrl+B` | Toggle the sidebar |
| `Alt+P` | Toggle the details pane |
| `Alt+Left` / `Right` / `Up` | Back, forward, up one folder |
| `Ctrl+L` | Type a path |
| `Ctrl+F` | Focus the search box |
| `Ctrl+H` | Show hidden items |
| `Ctrl+N` / `Ctrl+Shift+N` | New text document / folder |
| `Ctrl+T` | New tab showing files |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+A`, `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | Selection and clipboard |
| `F2` | Rename |
| `Delete` / `Shift+Delete` | Trash / delete permanently |
| `Ctrl+Z` | Undo the last file operation |
| `Alt+Enter` | Properties |
| `Ctrl+S`, `Ctrl+W` | Save, close the tab |
| `Tab` / `Shift+Tab` | Indent or outdent |
| `Ctrl+/` | Toggle a line comment |
| `F5` | Reload the folder |
| `/` or `?` | Shortcut help |

## How it stays fast

- Folders are read, searched, copied and decoded on worker threads; the window
  never waits on the disk.
- Row text is shaped once and cached, so scrolling a folder with tens of
  thousands of entries repaints without re-laying-out any text.
- The Markdown preview parses to a block tree and shapes each block once; typing
  debounces the re-render by 120 ms, so the editor never waits on the preview.
- Only the rows that intersect the viewport are built, in all three views.
- The status bar shows the real cost of each update, so the claim is checkable.

## Design

The interface is monochrome by construction: every colour in the palette has
identical red, green and blue channels, and a unit test fails the build if that
ever stops being true. The only exception is the tint on destructive actions.
Spacing comes from one scale, so panels, rows, dialogs and the status bar all
share the same rhythm.

## Licence

MIT. See [LICENSE](LICENSE).
