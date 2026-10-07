# Rhumb TODO

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
- [x] `RHUMB_BENCH` split into named sections, so a slow frame can be
      attributed rather than merely counted.
- [x] `app.rs` split into modules (toolbar, sidebar, list, settings, status, ...).
- [x] Folder tabs, each with its own history, selection and open files.
- [x] Archives (zip, tar, tar.gz) browsable as read-only folders, with extract.
- [x] Per-folder name index with ranked search, refreshed by the watcher.
- [x] Multi-cursor: Alt+Click, Ctrl+D, Ctrl+Shift+L, column selection.
- [x] Rope-backed buffer; edits and undo no longer scale with file size.
- [x] Icon font, settings window, size slider, quick-access unpinning.
- [x] Replace and replace-all in the find bar (Ctrl+H): a second field, with the
      edit applied in one undo step.
- [x] File-op collision UI: replace (overwrite/merge), skip, or keep both, with
      apply-to-all, resolved before the job starts.

## Now - small, high value, no COM

- [ ] Registry shell verbs - Directory\Background, Drive, Folder\Open\command.
      ~40 lines, opt-in + uninstaller.
- [ ] Go to line (Ctrl+G) - a line number box, reusing the find bar's row.

## Performance

- [ ] The sidebar is the largest single part of a frame at 1.1-2.2 ms, and it
      does not grow with the file, so it is a constant tax rather than a scaling
      problem. Worth a look before anything else.
- [ ] Mark the editor's work in the bench sections, so `central` can be split
      into shaping, painting and the find bar.

## Structural - unblocks everything below

- [x] Loc enum + Backend trait - `Loc::of` classifies a path as Dir, Archive,
      Recycle or ThisPc, and `Backend::list` lists it; `read_dir` dispatches
      through it. (`Entry.kind` instead of `is_dir` is not done: the shell
      places are modelled as folders, so the rename is cosmetic.)

## Reimplement

- [x] Recycle Bin - parse $I* from C:\$Recycle.Bin\<SID>\, browse and restore.
- [x] This PC - user folders + drives, WNetGetConnection for mapped drives.
- [x] File-op collision UI - replace/merge/skip/keep both + apply-to-all.
- [x] Pause, rate, ETA, retry on ERROR_SHARING_VIOLATION, cross-volume move.
- [x] 7z and RAR archives (read-only); writing into a zip.
- [x] Thumbnails hybrid - image crate plus the Windows shell handler (video/PDF/RAW).
- [x] Long paths - \\?\ normalization at the OS boundary (the index walk still
      uses plain paths).

## Chores - cheap, just unimplemented

- [x] Real per-type icons (Windows shell), IME composition, name-index persistence.
- [x] Multi-rename, junction/symlink creation; Extract here already existed.
- [x] Better breadcrumbs (chevron separators), real nav icons.
- [x] Editor IME composition.
- [x] Editor code folding.
- [ ] Editor gaps: language server, tree-sitter highlighting.
- [x] Dual-pane view.
- [x] Persist the name index between sessions.
- [x] Linux: clipboard and single-window handling (a Linux CI job is in place;
      not runtime-tested on this machine).
