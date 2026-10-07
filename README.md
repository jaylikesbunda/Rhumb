# Rhumb

A fast file explorer with a built-in code editor and live Markdown
preview

<img width="1628" height="918" alt="image" src="https://github.com/user-attachments/assets/16240ac0-7caa-4dd3-8a2d-41c726a2e08d" />


- **One tool for browsing and editing.** Open a file from the list and edit it
  in place. Markdown gets a live preview in the same tab.
- **Cost per keystroke does not grow with the file.** Edits and scrolling work
  on a rope and only touch the visible lines.
- **Searching never blocks the window.** Folders are read, indexed and searched
  on worker threads.
- **Small and self-contained.** One executable, no installer needed, no
  telemetry.

## Install

Download a build from the releases page:

| File | |
|:--|:--|
| `rhumb-<version>-setup.exe` | Windows installer (per user, no admin needed) |
| `rhumb-<version>-x64.msi` | Windows installer (MSI) |
| `rhumb-<version>-windows-portable.zip` | Windows, no install |

Settings are stored in `%LOCALAPPDATA%\rhumb\prefs.txt`. Delete it to reset.

<details>
<summary><b>Features</b></summary>

&nbsp;

**Browsing**
- Folder tabs, each with its own place, history, selection and open files
- Dual-pane view: two folder lists side by side (`Ctrl+Shift+D`), so files can
  be copied or moved between them
- Details, compact list and large-icon views, with real Windows shell icons and
  thumbnails (images, and video, PDF and RAW through the shell)
- Group by name, type, modified or size, with kind, date and size filters
- Breadcrumb path bar with chevron separators, sortable resizable columns,
  quick access with pinning
- Search as you type: this folder, or everything below it, cancellable; the
  per-folder name index is saved and reloaded between sessions
- ZIP, TAR, TAR.GZ, 7z and RAR archives open like folders, with extract from the
  context menu; a ZIP can be added to or deleted from in place
- The Recycle Bin and This PC as browsable locations, with restore
- Copy, cut, paste, batch rename, symlink/junction creation, trash, delete,
  zip, properties, undo, drag and drop
- A collision dialog for copy and move: replace/merge, skip or keep both, with
  apply-to-all; jobs can be paused, with throughput, an estimated time left and
  retries on a briefly locked file
- Long-path (`\\?\`) support, so trees past Windows' 260-character limit are
  reachable
- Native file clipboard, so files move to and from Explorer

**Editing**
- Rope-backed editor that stays responsive on files with 100,000+ lines
- Multi-cursor: `Alt+Click`, `Ctrl+D` (next occurrence), `Ctrl+Shift+L`
  (all occurrences), column selection, one undo step per multi-edit
- Syntax highlighting, bracket matching, auto-close, comment toggling, soft
  wrap, find and replace, and code folding
- IME composition, so input methods type at the caret
- Live Markdown preview beside the source, with optional scroll lock; local,
  remote and SVG images are drawn, not printed as paths

**Interface**
- Settings window, zoom slider, three views, light touch of chrome
- Accessible names and roles across the toolbar, list, tabs, sidebar, status
  bar and editor
- Phosphor icon set

</details>

<details>
<summary><b>Performance</b></summary>

&nbsp;

Frame costs measured on the release build with 20,000 to 100,000 line files,
including layout and tessellation (GPU time not included), on one Windows 11
desktop under normal load. Medians, in milliseconds. A frame at 240 fps is
4.2 ms.

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

</details>

<details>
<summary><b>Keyboard shortcuts</b></summary>

&nbsp;

| Shortcut | Action |
|:---------|:-------|
| `Ctrl+1` / `2` / `3` | Details / list / large-icon view |
| `Ctrl+B`, `Alt+P` | Toggle sidebar / details pane |
| `Alt+Left` / `Right` / `Up` | Back, forward, up |
| `Ctrl+L`, `Ctrl+F` | Type a path / focus search |
| `Ctrl+H` | Show hidden items (in the editor: find and replace) |
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
| `Ctrl+Shift+D` | Dual-pane: two folder lists |
| `Ctrl+Shift+V` | Cycle the list layout |
| `F2` over a selection | Batch rename |
| `Ctrl+,` | Settings |
| `F11` | Focus mode (editor) |
| `F5` | Reload the folder |
| `/` or `?` | Shortcut help |

</details>

<details>
<summary><b>Limits</b></summary>

&nbsp;

- Windows is the primary platform. Linux builds and is covered by CI, with a
  clipboard and single-instance implementation, but it gets less real-world
  testing; macOS is not supported.
- The editor has no language server or tree-sitter highlighting.
- Only ZIP archives can be written; TAR, 7z and RAR are read-only.
- No shell registration as the default folder handler.
- A single line of a few hundred kilobytes to a megabyte is still shaped whole,
  so a frame over one costs far more than a normal file. Ordinary long lines
  are fine.

</details>

<details>
<summary><b>Building from source</b></summary>

&nbsp;

Needs Rust 1.88 or newer.

    cargo build --release
    cargo test
    cargo clippy --all-targets

`tools/build_msi.sh` builds the installer (needs WiX v3 on `PATH`).

**Releasing:** run the **release** workflow from the Actions tab and enter a
version such as `0.2.0`. It stamps that version into the build, produces the
MSI and portable zip, tags `v<version>` and publishes the release.

</details>

## Licence

MIT. See [LICENSE](LICENSE).
