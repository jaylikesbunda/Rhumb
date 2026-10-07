# Changelog

## 0.2.0

### Added
- An About section in Settings, with the icon, the name, the version and a link to the GitHub page.
- A close button on the details pane that shows the selected file or folder. A matching button in the status bar, beside the size slider, brings it back; Settings and Alt+P still toggle it too.
- The icon is embedded in the executable.
- The Markdown preview draws images, local files and remote ones alike, including SVG badges, instead of printing their path. It also colours links, renders superscript and subscript, gives tables GitHub-style borders and a tinted header row, and turns `<details>` blocks into collapsible sections.
- An image is drawn at the size an `<img>` tag asks for, or at its own natural size, and is never enlarged past its own pixels; only an oversized one is shrunk to fit.
- A clear button in the search box, which empties it and puts the caret back so a new search can be typed at once.
- A Details pane button in the toolbar, so a pane closed from its own header always has an obvious way back, even on a window too narrow for the status bar's own switch.
- A collision dialog for copy and move: when a name is already taken, choose Replace (overwrite a file, or merge into a folder), Skip, or Keep both, with an option to apply the choice to every remaining conflict.
- A pause/resume control for copy and move, with throughput and an estimated time left beside the running job, and retries on a file that is briefly locked by another program.
- Long-path support: paths past Windows' 260-character limit are reached through the `\\?\` form, so deep trees can be browsed, read and written.
- The editor accepts input-method (IME) composition, showing the preedit at the caret and inserting the committed text.
- The folder name index is saved and loaded between sessions, so a large tree is searchable at once on the next launch instead of being walked again.
- Accessible names and roles on the toolbar, list rows, tabs, sidebar, status bar and editor, so a screen reader can navigate the chrome.
- Real per-type icons from the Windows shell in the file list, with the built-in glyph as a fallback.
- The Recycle Bin as a browsable location, with Restore and Delete permanently (double-click restores).
- Read-only 7z and RAR archives, browsed as folders beside the existing zip and tar support.
- Writing into a zip: pasting or dropping files into an open zip adds them, and deleting an entry removes it, with the archive rewritten safely beside itself.
- A dual-pane view: two folder lists side by side (toolbar button or Ctrl+Shift+D), each with its own place and selection, so files can be copied or moved between them.
- List grouping (by name, type, modified or size) and kind/date/size filters, with the choices remembered between sessions.
- Multi-rename with a pattern (`{name}`, `{ext}`, `{n}`, padding) and a live preview, and creating symbolic links or junctions.
- Thumbnails from the Windows shell for files the image crate cannot decode (video, PDF, RAW), falling back to the per-type icon.
- "This PC" as a browsable place, and a chevron between breadcrumb segments instead of a plain separator.
- A Linux clipboard and single-instance implementation, plus a Linux CI job.
- Code folding in the editor: brace and indentation blocks, a gutter chevron, and fold-all/unfold-all.
- More accessible controls (Settings switches, the find bar, the editor scrollbar), and search works inside the Recycle Bin and This PC without walking the disk.

### Changed
- The icon is redrawn: a proper folder with a sloped tab and rounded corners, and a much thinner outline. Small sizes (16 to 48 px) are drawn separately so the line weight matches at every size, with extra 20 and 40 px sizes for scaled displays.
- The window and taskbar now use the icons embedded in the executable at the exact size Windows asks for, rather than one bitmap it rescales. They now match what Explorer shows.
- The icon generator (`tools/make_icon.py`) uses Pillow.
- The maximise and restore buttons are a little smaller, so they match minimise and close.
- Linked scrolling between the editor and the Markdown preview is rebuilt around an exact layout map. The preview lays every block out once, so the line-to-pixel map never moves under the reader and the two panes stay in step; the map is found by binary search rather than by walking every block.

### Fixed
- The folder tab in the old icon was placed in the wrong units at every size but 256 px, so the icon looked different in different places.
- A panic is written to `rhumb.log` before the process exits, so a crash can be diagnosed instead of vanishing silently.

### Performance
- Faster start: graphics are initialised for Direct3D 12 only on Windows instead of probing every backend.
- Faster start: the drive list is read by a background worker instead of before the first frame, so a slow or sleeping disk no longer delays the window.
- Faster close: the app exits as soon as its settings are saved instead of waiting for the graphics device and workers to wind down.

## 0.1.0

First release.
