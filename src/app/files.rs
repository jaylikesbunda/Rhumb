//! What is done to files: clipboard, copy, move, delete, rename, create and undo.

use super::*;

impl Xplor {
    /// Routes this frame's clipboard shortcut to the file list.
    ///
    /// Ctrl+C, Ctrl+X and Ctrl+V never arrive as key presses: the window layer
    /// turns them into these events and drops the key, which is what a
    /// `TextEdit` needs in order to reach the system clipboard. Asking
    /// `consume_key` for them therefore finds nothing, and the file list's copy
    /// and paste quietly did nothing at all.
    ///
    /// The editor draws before this and takes the same events when it has
    /// focus, so the caret wins over the file list, which is the order every
    /// other editor uses.
    pub(super) fn handle_clipboard_events(&mut self, ctx: &Context) {
        let mut copy = false;
        let mut cut = false;
        let mut paste = false;
        for ev in ctx.input(|i| i.events.clone()) {
            match ev {
                egui::Event::Copy => copy = true,
                egui::Event::Cut => cut = true,
                egui::Event::Paste(_) => paste = true,
                _ => {}
            }
        }
        if self.ed_took_clipboard {
            return;
        }
        if copy {
            self.copy_selection(false);
        } else if cut {
            self.copy_selection(true);
        } else if paste {
            self.paste();
        }
    }

    /// Says so and returns true when `paths` are in an archive, which cannot be
    /// changed: nothing is ever written into one.
    pub(super) fn refuse_in_archive(&mut self, paths: &[&Path]) -> bool {
        if paths.iter().any(|p| archive::is_virtual(p)) {
            self.toast("Archives are read-only: extract them to change anything".into());
            return true;
        }
        false
    }

    /// The paths with any that are inside an archive brought out to real files, which
    /// is all the clipboard and a copy can use. `None` if one could not be.
    pub(super) fn materialized(&mut self, paths: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            match archive::materialize(&p) {
                Ok(real) => out.push(real),
                Err(e) => {
                    self.toast_err(format!(
                        "Cannot read {} from the archive: {e}",
                        display_name(&p)
                    ));
                    return None;
                }
            }
        }
        Some(out)
    }

    pub(super) fn copy_selection(&mut self, cut: bool) {
        let mut paths = self.target_paths();
        if paths.is_empty() {
            return;
        }
        if paths.iter().any(|p| archive::is_virtual(p)) {
            // Copying out of an archive is fine; cutting would remove from it.
            if cut {
                self.toast("Archives are read-only: extract them to change anything".into());
                return;
            }
            let Some(real) = self.materialized(paths) else {
                return;
            };
            paths = real;
        }
        paths.sort();
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let text = paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = cb.set_text(text);
        }
        // Explorer cannot read the text above, so offer CF_HDROP too. Cut or
        // copied travels in "Preferred DropEffect".
        crate::clip::write_hdrop(&paths, cut);
        let n = paths.len();
        self.clip = Some(Clipboard { paths, cut });
        self.toast(format!(
            "{} {n} item(s)",
            if cut { "Cut" } else { "Copied" }
        ));
    }

    pub(super) fn paste(&mut self) {
        // Our own copy wins; otherwise take Explorer's files (CF_HDROP) or,
        // failing that, lines of text that name files.
        let (sources, cut): (Vec<PathBuf>, bool) = match self.clip.clone() {
            Some(clip) if clip.paths.iter().any(|p| p.exists()) => (clip.paths, clip.cut),
            _ => match crate::clip::read_hdrop() {
                Some(hdrop) => (hdrop.paths, hdrop.cut),
                None => (self.system_paths(), false),
            },
        };
        if sources.is_empty() {
            self.toast("Clipboard holds no files".into());
            return;
        }
        self.start_transfer(sources, self.cwd.clone(), cut);
        if cut {
            self.clip = None;
        }
    }

    /// Kicks off a copy or move, recording it for undo.
    pub(super) fn start_transfer(&mut self, sources: Vec<PathBuf>, dest: PathBuf, cut: bool) {
        if sources.is_empty() {
            return;
        }
        if archive::is_virtual(&dest) {
            self.toast("Archives are read-only: extract them to change anything".into());
            return;
        }
        let mut sources = sources;
        if sources.iter().any(|p| archive::is_virtual(p)) {
            if cut {
                self.toast("Archives are read-only: copy out of one, do not move".into());
                return;
            }
            let Some(real) = self.materialized(sources) else {
                return;
            };
            sources = real;
        }
        // Work out where everything lands so undo knows the reverse mapping.
        let mut pairs = Vec::with_capacity(sources.len());
        for src in &sources {
            let Some(name) = src.file_name() else {
                continue;
            };
            // Never overwrite: a colliding name gets " (2)" and friends.
            let target = dest.join(name);
            let target = if target == *src || target.exists() {
                fs_model::unique_dest(&target)
            } else {
                target
            };
            pairs.push((src.clone(), target));
        }
        if pairs.is_empty() {
            return;
        }
        let id = self.ids.next();
        let from: Vec<PathBuf> = pairs.iter().map(|(s, _)| s.clone()).collect();
        let to: Vec<PathBuf> = pairs.iter().map(|(_, t)| t.clone()).collect();
        let job = ops::start_transfer_pairs(self.tx.clone(), id, pairs.clone(), cut);
        let _ = from;
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(if cut {
            Undo::Moved { items: pairs }
        } else {
            Undo::Copied { items: to }
        });
    }

    /// Moves items that were dropped onto a folder.
    pub(super) fn drop_onto(&mut self, paths: Vec<PathBuf>, dest: &Path, copy: bool) {
        let mut paths = paths;
        paths.retain(|p| p.parent() != Some(dest));
        if paths.is_empty() {
            return;
        }
        self.start_transfer(paths, dest.to_path_buf(), !copy);
    }

    /// Puts one path on the clipboard as text, which is what a shell expects.
    pub(super) fn copy_as_path(&mut self, path: &Path) {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            self.toast_err("No clipboard available".into());
            return;
        };
        if cb.set_text(path.to_string_lossy().into_owned()).is_err() {
            self.toast_err("Could not write to the clipboard".into());
        }
    }

    /// Opens a terminal in the folder that holds `path`.
    ///
    /// Each platform gets the terminal it actually ships with, because a wrong
    /// guess would silently do nothing.
    pub(super) fn open_in_terminal(&mut self, path: &Path) {
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        };
        let candidates: &[(&str, &[&str])] = if cfg!(windows) {
            &[
                ("wt.exe", &["-d", "."]),
                ("powershell.exe", &["-NoExit", "-Command", "Set-Location ."]),
                ("cmd.exe", &["/k", "cd ."]),
            ]
        } else if cfg!(target_os = "macos") {
            &[("open", &["-a", "Terminal", "."])]
        } else {
            &[
                ("x-terminal-emulator", &["./"]),
                ("gnome-terminal", &["./"]),
                ("konsole", &["./"]),
                ("xterm", &["./"]),
            ]
        };
        let mut launched = false;
        for (program, args) in candidates {
            let r = std::process::Command::new(program)
                .args(*args)
                .current_dir(&dir)
                .spawn();
            if r.is_ok() {
                launched = true;
                break;
            }
        }
        if !launched {
            self.toast_err("No terminal found on this system".into());
        }
    }

    /// Compresses the selection into a zip beside it.
    pub(super) fn start_zip(&mut self) {
        let sources: Vec<PathBuf> = self.sel.iter().cloned().collect();
        if sources.is_empty() {
            return;
        }
        if sources.iter().any(|p| archive::is_virtual(p)) {
            self.toast("Extract first: a file inside an archive cannot be compressed".into());
            return;
        }
        // The archive lands next to the first item, like Explorer does.
        let dest_dir = sources[0]
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.cwd.clone());
        let id = self.ids.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ops::start_zip(self.tx.clone(), id, sources.clone(), dest_dir, cancel);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(Undo::Created(Vec::new()));
    }

    /// Extracts an archive, or something inside one, into a new folder next to the
    /// archive named for it, on a job of its own.
    pub(super) fn start_extract(&mut self, path: &Path) {
        let Some(archive::Inside { archive, inner }) = archive::split(path) else {
            return;
        };
        let Some(parent) = archive.parent() else {
            return;
        };
        let stem = archive
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".to_owned());
        // A `.tar.gz` has its second extension left on its stem.
        let stem = stem.strip_suffix(".tar").unwrap_or(&stem).to_owned();
        let dest = fs_model::unique_dest(&parent.join(stem));
        let id = self.ids.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ops::start_extract(self.tx.clone(), id, archive, inner, dest.clone(), cancel);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(Undo::Created(vec![dest]));
    }

    pub(super) fn system_paths(&self) -> Vec<PathBuf> {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return Vec::new();
        };
        let Ok(text) = cb.get_text() else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| {
                let p = PathBuf::from(l.trim());
                p.exists().then_some(p)
            })
            .collect()
    }

    pub(super) fn delete_selection(&mut self, permanent: bool) {
        let paths = self.target_paths();
        if paths.is_empty() {
            return;
        }
        if paths.iter().any(|p| archive::is_virtual(p)) {
            self.toast("Archives are read-only: extract them to change anything".into());
            return;
        }
        if permanent {
            self.dialog = Dialog::ConfirmDelete { paths };
            return;
        }
        // Default: the OS recycle bin, so nothing is lost by accident.
        match ops::send_to_trash(&paths) {
            Ok(()) => {
                self.toast(format!("Moved {} item(s) to trash", paths.len()));
                self.sel.clear();
                self.request_listing();
            }
            Err(e) => self.toast_err(e),
        }
    }

    pub(super) fn start_permanent_delete(&mut self, paths: Vec<PathBuf>) {
        let id = self.ids.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ops::start_permanent_delete(self.tx.clone(), id, paths.clone(), cancel);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(Undo::Created(paths));
        self.sel.clear();
    }

    pub(super) fn start_rename(&mut self, path: &Path) {
        if self.refuse_in_archive(&[path]) {
            return;
        }
        self.dialog = Dialog::Rename {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        };
    }

    pub(super) fn apply_rename(&mut self, path: &Path, name: &str) {
        let name = name.trim();
        let Some(parent) = path.parent() else { return };
        let target = parent.join(name);
        if target == path {
            return;
        }
        if target.exists() {
            self.toast_err(format!("{name} already exists"));
            return;
        }
        match std::fs::rename(path, &target) {
            Ok(()) => {
                self.undo = Some(Undo::Renamed {
                    from: path.to_path_buf(),
                    to: target.clone(),
                });
                self.undo_stack.push(self.undo.clone().expect("just set"));
                self.toast(format!("Renamed to {name}"));
                if let Some(doc) = self.doc_mut()
                    && doc.path == path
                {
                    doc.path = target;
                }
                self.request_listing();
            }
            Err(e) => self.toast_err(format!("Rename failed: {e}")),
        }
    }

    pub(super) fn apply_create(&mut self, dir: &Path, name: &str, folder: bool) {
        if self.refuse_in_archive(&[dir]) {
            return;
        }
        let name = name.trim();
        let target = dir.join(name);
        if target.exists() {
            self.toast_err(format!("{name} already exists"));
            return;
        }
        let r = if folder {
            std::fs::create_dir(&target)
        } else {
            std::fs::write(&target, b"")
        };
        match r {
            Ok(()) => {
                self.undo = Some(Undo::Created(vec![target.clone()]));
                self.undo_stack.push(self.undo.clone().expect("just set"));
                self.request_listing();
                if folder {
                    self.toast(format!("Created folder {name}"));
                } else {
                    self.toast(format!("Created {name}"));
                    self.open_path(&target);
                }
            }
            Err(e) => self.toast_err(format!("Could not create {name}: {e}")),
        }
    }

    /// Rolls back the last operation, the way Explorer does with Ctrl+Z.
    pub(super) fn undo(&mut self) {
        let Some(action) = self.undo.take() else {
            self.toast("Nothing to undo".into());
            return;
        };
        let mut problems: Vec<String> = Vec::new();
        match action {
            Undo::Created(paths) => {
                for p in &paths {
                    let r = if p.is_dir() {
                        std::fs::remove_dir_all(p)
                    } else {
                        std::fs::remove_file(p)
                    };
                    if let Err(e) = r {
                        problems.push(format!("{}: {e}", display_name(p)));
                    }
                }
                self.toast("Undo: removed the new item(s)".into());
            }
            Undo::Renamed { from, to } => {
                if let Err(e) = std::fs::rename(&to, &from) {
                    problems.push(format!("{}: {e}", display_name(&to)));
                } else if let Some(doc) = self.doc_mut()
                    && doc.path == to
                {
                    doc.path = from;
                }
                self.toast("Undo: rename reverted".into());
            }
            Undo::Moved { items } => {
                for (src, dst) in items {
                    // Move the destination back to where it came from.
                    let r = if std::fs::rename(&dst, &src).is_ok() {
                        Ok(())
                    } else {
                        ops::move_now(&dst, &src)
                    };
                    if let Err(e) = r {
                        problems.push(format!("{}: {e}", display_name(&dst)));
                    }
                }
                self.toast("Undo: move reverted".into());
            }
            Undo::Copied { items } => {
                for p in &items {
                    let r = if p.is_dir() {
                        std::fs::remove_dir_all(p)
                    } else {
                        std::fs::remove_file(p)
                    };
                    if let Err(e) = r {
                        problems.push(format!("{}: {e}", display_name(p)));
                    }
                }
                self.toast("Undo: copies removed".into());
            }
        }
        self.request_listing();
        if problems.is_empty() {
            if let Some(next) = self.undo_stack.pop() {
                self.undo = Some(next);
            }
        } else {
            self.toast_err(format!("Undo incomplete: {}", problems.join("; ")));
        }
    }

    /// Records the operation a worker just finished, if it is undoable.
    pub(super) fn settle_pending_undo(&mut self) {
        if let Some(action) = self.pending_undo.take() {
            self.undo_stack.push(action);
            self.undo = self.undo_stack.last().cloned();
        }
    }

    /// Handles files dragged in from the OS, plus our own drag-and-drop
    /// payloads (paths encoded as a text list, the same format Explorer uses).
    pub(super) fn handle_file_drop(&mut self, ctx: &Context) {
        let mut dropped: Vec<PathBuf> = Vec::new();
        ctx.input(|i| {
            for f in &i.raw.dropped_files {
                dropped.push(f.path().to_path_buf());
            }
        });

        // Our own payload: newline separated paths, matching Explorer.
        if dropped.is_empty() {
            let mut text = String::new();
            ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Paste(t) = ev {
                        text.push_str(t);
                    }
                }
            });
            if !text.is_empty() {
                dropped = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .collect();
            }
        }
        if dropped.is_empty() {
            return;
        }

        // Dropped on a folder row or the sidebar: move into it. Dropped on
        // empty space: copy into the current folder.
        let target = self.drop_target.take();
        match target {
            Some(folder) if folder.is_dir() => {
                let copy = ctx.input(|i| i.modifiers.ctrl || i.modifiers.command);
                self.drop_onto(dropped, &folder, copy);
            }
            _ => {
                for path in dropped {
                    if path.is_dir() {
                        self.navigate(&path);
                    } else {
                        if let Some(parent) = path.parent() {
                            self.navigate(parent);
                        }
                        self.open_path(&path);
                    }
                }
            }
        }
    }

    // ---- watching ---------------------------------------------------------------
}
