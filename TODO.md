# Xplor TODO

Backlog, roughly in priority order. Items are unchecked until done.

## Done

### The editor

Replaced `egui_code_editor` with a from-scratch visible-only editor in
`src/codeedit/`. Frame cost is now proportional to what is on screen, not to
the size of the file, and the whole frame is flat across a 40x range of file
sizes.

- [x] Visible-only shaping; nothing off screen is ever laid out.
- [x] Caret, selection, gutter, click and drag hit-tests that agree with each
      other. Three separate off-by-N bugs lived here.
- [x] Syntax highlighting, gated on the language actually being recognised.
- [x] Undo/redo, clipboard, comment toggle, auto-close pairs, smart indent.
- [x] Caret movement by character, word, line, screenful and document.
- [x] Double click for a word, triple click for the line.
- [x] Smart Home; Ctrl+Backspace / Ctrl+Delete; Ctrl+D; Ctrl+Shift+K.
- [x] Scrollbar: drag the thumb, click the track to page, hover to brighten.
- [x] Sideways scrolling, so a line longer than the pane is readable.
- [x] Find in the document: match count, match case, whole word, Enter and
      F3 to step, matches marked under the text.
- [x] Ln / Col / selection count in the status bar.
- [x] `src/codeedit/behaviour.rs` drives the editor through real input events
      and asserts on both the buffer and the drawn geometry; a fuzzer over
      random key sequences guards against panics and out-of-range carets.

### Elsewhere

- [x] Clipboard CF_HDROP - read + write. Fixes all three cross-app paste
      failures.
- [x] Fix the tofu glyph on the New button.
- [x] Refresh button in the toolbar (F5 already worked).
- [x] Single-instance broker - named mutex + signal file, forward the path,
      exit. Fixes N-windows-N-histories.
- [x] `Doc::dirty` cached against the document version. It hashed the whole
      buffer, twice a frame, and was the largest cost in the frame after the
      editor itself.
- [x] `XPLOR_BENCH` split into named sections, so a slow frame can be
      attributed rather than merely counted.
- [x] `app.rs` split into modules (toolbar, sidebar, list, settings, status, ...).
- [x] Folder tabs, each with its own history, selection and open files.
- [x] Archives (zip, tar, tar.gz) browsable as read-only folders, with extract.
- [x] Per-folder name index with ranked search, refreshed by the watcher.
- [x] Multi-cursor: Alt+Click, Ctrl+D, Ctrl+Shift+L, column selection.
- [x] Rope-backed buffer; edits and undo no longer scale with file size.
- [x] Icon font, settings window, size slider, quick-access unpinning.

## Now - small, high value, no COM

- [ ] Registry shell verbs - Directory\Background, Drive, Folder\Open\command.
      ~40 lines, opt-in + uninstaller.
- [ ] Replace and replace-all in the find bar. The bar and the search are
      built; only the second field and the edit are missing.
- [ ] Go to line (Ctrl+G) - a line number box, reusing the find bar's row.

## Performance

- [ ] The sidebar is the largest single part of a frame at 1.1-2.2 ms, and it
      does not grow with the file, so it is a constant tax rather than a scaling
      problem. Worth a look before anything else.
- [ ] Mark the editor's work in the bench sections, so `central` can be split
      into shaping, painting and the find bar.

## Structural - unblocks everything below

- [ ] Loc enum + Backend trait - Dir(PathBuf) / Shell(ShellId), Entry.kind
      instead of is_dir: bool. Touches fs_model.rs + tree.rs.

## Reimplement

- [ ] Recycle Bin - parse $I* from C:\$Recycle.Bin\<SID>\. Gets you restore
      and search.
- [ ] This PC - WNetGetConnection + WM_DEVICECHANGE, no system-folder noise.
- [ ] File-op collision UI - merge/replace/skip/rename/compare + apply-to-all.
- [ ] Pause, rate, ETA, retry on ERROR_SHARING_VIOLATION, cross-volume move.
- [ ] 7z and RAR archives; writing into archives.
- [ ] Thumbnails hybrid - image crate up to ~12 formats, shell handler for
      RAW/PDF/video.
- [ ] Long paths - \\?\ normalization throughout.

## Chores - cheap, just unimplemented

- [ ] Real per-type icons, grouping, date/size/kind filters.
- [ ] Multi-rename, junction/symlink creation, Extract here.
- [ ] Better breadcrumbs (separator vs. arrow glyphs), real nav icons.
- [ ] Editor gaps: language server, tree-sitter highlighting, code folding,
      IME composition.
- [ ] Dual-pane view.
- [ ] Persist the name index between sessions.
- [ ] Linux: clipboard and single-window handling, then test builds.
