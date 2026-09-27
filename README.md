# Xplor

A dark file explorer with a built-in text and Markdown editor. Native Rust and
[egui](https://github.com/emilk/egui) — one binary, nothing to install.

![Xplor](assets/screenshot.png)

## What it does

- Lazy Explorer-style tree, clickable path segments, sortable drag-to-resize
  columns
- Tabs, like Windows 11 Explorer
- Three views: details, compact list, large icons with image thumbnails
- Search as you type, in this folder or everything below it, cancellable and
  never blocking the window
- Text and code editing: syntax highlighting, bracket matching, auto-close,
  comment toggling, soft wrap
- Live Markdown preview beside the source
- Details pane: image thumbnail, or the first lines of a text file
- Properties, including a background size count for folders
- Copy, cut, paste, rename, trash, permanent delete, ZIP, copy path, open in
  terminal, undo
- Drag and drop, to move or (`Ctrl`) copy

Copy and paste speak the same clipboard formats as Explorer, so files move
between Xplor and any other program in both directions.

Folders are read, searched and copied on worker threads, and only the rows
intersecting the viewport are built. The status bar reports the measured cost of
each update, so you can check that claim yourself.

## Why not just use Explorer

One binary, about 15 MB, no runtime, no installer required, no ads, no sync nag,
no tips, no telemetry. It starts instantly and holds a folder with tens of
thousands of entries without breaking a sweat.

And it has something Explorer does not have at all: opening a `.md` file puts
the source and a rendered preview in the same tab, with the editor already
there. Opening a `.rs` or `.py` file gives you a real editor instead of
Notepad. Explorer has no answer to any of that.

## Measured

One machine, one method, stated so you can check it. Not a benchmark suite.

| | Xplor | explorer.exe |
|:--|--:|--:|
| On disk | 14.7 MB | 3.3 MB (system component) |
| Cold start to first window | _unmeasured_ | _unmeasured_ |
| Idle CPU, one folder open | _unmeasured_ | _unmeasured_ |
| Memory, one folder open | _unmeasured_ | _unmeasured_ |
| Open a folder of ~20,000 files | _unmeasured_ | _unmeasured_ |
| Runtime to install | none | — |

The blank cells are deliberate. Run `tools/measure.ps1` from a fresh login with
nothing else open, and read the folder-open figure off Xplor's own status bar,
median of five. An earlier run on a busy machine — a browser playing video, a
dozen tabs — put Explorer at 10.6% CPU and 341 MB, but that was media decode and
thumbnail work for the browser's windows, not Explorer browsing files. Those
numbers are not in the table because they would not survive a second look.

Two things worth knowing about the comparison when you fill it in.
`explorer.exe` is one shared process for the whole desktop: it draws the
desktop and the taskbar and hosts every registered shell extension, so part of
its memory is work Xplor does not do and does not need to. The comparison is
fair for browsing files; it is not a claim that Explorer does less work. And
Xplor's numbers are reproducible by you, because the status bar prints the cost
of every update as it happens.

## Not wired up yet

Everything above works on its own. These are the edges where Xplor doesn't yet
reach the rest of Windows:

- **No shell registration.** Xplor is not yet the handler for folders, drives,
  or the desktop right-click, so double-clicking a folder elsewhere on the
  system still opens Explorer.
- **No extraction.** It writes ZIPs; it does not unpack ZIP, 7z, RAR or tar.
- **Thumbnails cover PNG, JPEG, GIF, BMP and WebP.** Everything else falls back
  to a generic icon.
- **The Recycle Bin cannot be browsed.** Items can be sent to it, but there is
  no view to restore from.
- **Paths over 260 characters are not handled yet.**

## Install

Take a build from the releases page, or compile it yourself.

| Platform | File |
|:---------|:-----|
| Windows  | `xplor-<version>-x64.msi` |
| Windows  | `xplor-<version>-windows-portable.zip` |
| Linux    | `xplor-<version>-x86_64.AppImage` — `chmod +x`, then run |

Settings persist in `%LOCALAPPDATA%\xplor\prefs.txt`. Delete it to reset.

## Build

Needs Rust 1.85 or newer.

    cargo build --release        # target/release/xplor, about 15 MB
    cargo test
    cargo clippy --all-targets

`tools/build_msi.sh` and `tools/make_appdir.sh` produce the installer and the
AppImage. Both need extra tooling on `PATH` (WiX v3, `appimagetool`).

`tools/measure.ps1` fills in the table above.

## Keys

| Shortcut | Action |
|:---------|:-------|
| `Ctrl+1` / `2` / `3` | Details / list / large-icon view |
| `Ctrl+Shift+V` | Cycle the view |
| `Ctrl+B`, `Alt+P` | Toggle sidebar / details pane |
| `Alt+Left` / `Right` / `Up` | Back, forward, up one folder |
| `Ctrl+L`, `Ctrl+F` | Type a path / focus search |
| `Ctrl+H` | Show hidden items |
| `Ctrl+N` / `Ctrl+Shift+N` | New text document / folder |
| `Ctrl+T`, `Ctrl+Tab` | Next / new tab |
| `Ctrl+A`, `C`, `X`, `V` | Selection and clipboard |
| `F2`, `Delete`, `Shift+Delete` | Rename, trash, delete permanently |
| `Ctrl+Z` | Undo the last file operation |
| `Alt+Enter` | Properties |
| `Ctrl+S`, `Ctrl+W` | Save, close the tab |
| `Tab` / `Shift+Tab`, `Ctrl+/` | Indent, toggle line comment |
| `F5` | Reload the folder |
| `/` or `?` | Shortcut help |

## Licence

MIT. See [LICENSE](LICENSE).
