# Xplor TODO

Backlog, roughly in priority order. Items are unchecked until done.

## Now — small, high value, no COM

- [ ] Clipboard CF_HDROP — read + write. Fixes all three cross-app paste
      failures. ~80 lines.
- [ ] Fix the ▼ tofu glyph on the New button.
- [ ] Refresh button in the toolbar (F5 already works).
- [ ] Registry shell verbs — Directory\Background, Drive, Folder\Open\command.
      ~40 lines, opt-in + uninstaller.
- [ ] Single-instance broker — mutex + pipe, forward the path, exit. Fixes
      N-windows-N-histories.

## Structural — unblocks everything below

- [ ] Loc enum + Backend trait — Dir(PathBuf) / Shell(ShellId), Entry.kind
      instead of is_dir: bool. Touches fs_model.rs + tree.rs.

## Reimplement

- [ ] Recycle Bin — parse $I* from C:\$Recycle.Bin\<SID>\. Gets you restore
      and search.
- [ ] This PC — WNetGetConnection + WM_DEVICECHANGE, no system-folder noise.
- [ ] File-op collision UI — merge/replace/skip/rename/compare + apply-to-all.
- [ ] Pause, rate, ETA, retry on ERROR_SHARING_VIOLATION, cross-volume move.
- [ ] Zip as a namespace — Loc::Zip(Arc<ZipArchive>, _), read-only.
- [ ] Thumbnails hybrid — image crate up to ~12 formats, shell handler for
      RAW/PDF/video.
- [ ] Long paths — \\?\ normalization throughout.

## Chores — cheap, just unimplemented

- [ ] Real per-type icons, grouping, date/size/kind filters.
- [ ] Multi-rename, junction/symlink creation, Extract here.
- [ ] Better breadcrumbs (separator vs. arrow glyphs), real nav icons.
- [ ] Split app.rs (5,610 lines).
