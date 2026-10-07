//! What is done to files: clipboard, copy, move, delete, rename, create and undo.

use super::*;

impl Rhumb {
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
    ///
    /// A destination name that is already taken is not silently renamed: the transfer
    /// is held and the collision dialog asks what to do, so "replace" and "merge" are
    /// reachable and the old never-overwrite behaviour is still one of the choices.
    pub(super) fn start_transfer(&mut self, sources: Vec<PathBuf>, dest: PathBuf, cut: bool) {
        if sources.is_empty() {
            return;
        }
        let mut sources = sources;
        if let Some(inside) = archive::split(&dest) {
            // A zip can be written into, the way Explorer edits a compressed
            // folder; every other archive stays read-only. A cut into one is a
            // copy: the source stays where it was, which is what Explorer does.
            if archive::kind_of(&inside.archive) != Some(archive::Kind::Zip) {
                self.toast("Archives are read-only: extract them to change anything".into());
                return;
            }
            // Something inside an archive has to be brought out before it can go
            // into another one.
            if sources.iter().any(|p| archive::is_virtual(p)) {
                let Some(real) = self.materialized(sources) else {
                    return;
                };
                sources = real;
            }
            self.start_add_to_zip(inside.archive, inside.inner, sources);
            return;
        }
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
        let dest_norm = fs_model::normalize(&dest);
        let mut ready = Vec::with_capacity(sources.len());
        let mut conflicts = Vec::new();
        for src in sources {
            let Some(name) = src.file_name() else {
                continue;
            };
            // Copying or moving a folder into itself would recurse forever.
            if src.is_dir() && dest_norm.starts_with(fs_model::normalize(&src)) {
                self.toast_err(format!("Cannot copy {} into itself", display_name(&src)));
                continue;
            }
            let target = dest.join(name);
            if target == src || target.exists() {
                conflicts.push(src);
            } else {
                ready.push((src, target));
            }
        }
        if conflicts.is_empty() {
            self.finish_transfer(ready, cut);
        } else {
            self.dialog = Dialog::Collision {
                ready,
                conflicts,
                dest_dir: dest,
                cut,
                apply_all: false,
            };
        }
    }

    /// Adds files into the open zip, on a worker; the finished message refreshes
    /// the listing. Nothing is recorded for undo: an archive rewrite has no
    /// simple inverse once it has happened.
    fn start_add_to_zip(&mut self, archive: PathBuf, inner: String, sources: Vec<PathBuf>) {
        let id = self.ids.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ops::start_add_to_zip(self.tx.clone(), id, archive, inner, sources, cancel);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            paused: false,
            job,
        });
    }

    /// Starts a transfer whose destinations are all decided, recording it for undo.
    pub(super) fn finish_transfer(&mut self, pairs: Vec<(PathBuf, PathBuf)>, cut: bool) {
        if pairs.is_empty() {
            return;
        }
        let id = self.ids.next();
        let to: Vec<PathBuf> = pairs.iter().map(|(_, t)| t.clone()).collect();
        let job = ops::start_transfer_pairs(self.tx.clone(), id, pairs.clone(), cut);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            paused: false,
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
            paused: false,
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
            paused: false,
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
            // Deleting from a zip rewrites the archive. A non-zip archive stays
            // read-only, and there is no recycle bin inside one, so both delete
            // routes take a zip entry out at once.
            let all_zips = paths.iter().all(|p| {
                archive::split(p)
                    .is_some_and(|i| archive::kind_of(&i.archive) == Some(archive::Kind::Zip))
            });
            if all_zips {
                self.start_remove_from_zip(paths);
            } else {
                self.toast("Archives are read-only: extract them to change anything".into());
            }
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

    /// Takes entries out of the zip(s) holding them, one worker per archive, and
    /// lets the finished message refresh the listing. There is no undo for an
    /// archive edit: the old bytes are gone once the archive is rewritten.
    fn start_remove_from_zip(&mut self, paths: Vec<PathBuf>) {
        let mut by_archive: std::collections::BTreeMap<PathBuf, Vec<String>> =
            std::collections::BTreeMap::new();
        for p in paths {
            if let Some(inside) = archive::split(&p) {
                by_archive
                    .entry(inside.archive)
                    .or_default()
                    .push(inside.inner);
            }
        }
        for (archive, entries) in by_archive {
            let id = self.ids.next();
            let cancel = Arc::new(AtomicBool::new(false));
            let job = ops::start_remove_from_zip(self.tx.clone(), id, archive, entries, cancel);
            self.jobs.push(ActiveJob {
                done_items: 0,
                total_items: 0,
                done_bytes: 0,
                total_bytes: 0,
                current: String::new(),
                paused: false,
                job,
            });
        }
        self.sel.clear();
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
            paused: false,
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
                let undo = Undo::Renamed {
                    from: path.to_path_buf(),
                    to: target.clone(),
                };
                self.undo_stack.push(undo.clone());
                self.undo = Some(undo);
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

    /// Opens the batch-rename dialog for a whole selection. A selection of one
    /// keeps the single-file dialog, which is what F2 on one item has always
    /// done; anything that cannot be renamed on disk is dropped first.
    pub(super) fn start_batch_rename(&mut self, paths: Vec<PathBuf>) {
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        if self.refuse_in_archive(&refs) {
            return;
        }
        let mut paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| !crate::recycle::is_item(p))
            .collect();
        // One item is the ordinary rename, not a one-line batch.
        if paths.len() == 1 {
            self.start_rename(&paths[0]);
            return;
        }
        if paths.is_empty() {
            return;
        }
        // A stable order, so the counter runs down the list the way the reader
        // sees it rather than in whatever order a hash set hands the paths over.
        paths.sort();
        self.dialog = Dialog::BatchRename {
            paths,
            pattern: "{name}".to_owned(),
            start: "1".to_owned(),
        };
    }

    /// Renames a whole selection at once from a pattern.
    ///
    /// Two phases, because one pass can fail on itself: renaming `1.txt` to
    /// `2.txt` while `2.txt` is still there either fails or clobbers it. Every
    /// source is moved to a temporary name of its own first, which empties the
    /// way, and only then is each put under its final name. A name that is
    /// already taken by something outside the batch, or a bad one, is skipped
    /// and reported rather than written over.
    pub(super) fn apply_batch_rename(
        &mut self,
        paths: &[PathBuf],
        pattern: &str,
        start_text: &str,
    ) {
        let start: usize = start_text.trim().parse().unwrap_or(1);
        let mut paths: Vec<PathBuf> = paths.to_vec();
        paths.sort();

        let mut problems: Vec<String> = Vec::new();
        let mut plans: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for (i, src) in paths.iter().enumerate() {
            let Some(parent) = src.parent() else { continue };
            let stem = src
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = src
                .extension()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = fs_model::batch_name(pattern, &stem, &ext, start + i);
            let name = name.trim().to_owned();
            if let Err(msg) = fs_model::validate_name(&name) {
                problems.push(format!("{}: {msg}", display_name(src)));
                continue;
            }
            let target = parent.join(&name);
            let target_norm = fs_model::normalize(&target);
            if target_norm == fs_model::normalize(src) {
                continue;
            }
            if !seen.insert(target_norm.clone()) {
                problems.push(format!(
                    "{}: {name} is used more than once",
                    display_name(src)
                ));
                continue;
            }
            plans.push((src.clone(), target));
        }
        // A target that is already on disk is fine only when what sits there is
        // another member of the batch that is about to move out of the way.
        // Anything else would be overwritten by the second phase, so it is
        // skipped and reported instead. Dropping one plan can leave another
        // without its swap partner, so this repeats until nothing more falls
        // away. A member that turned out to be unchanged still counts as an
        // obstacle here rather than as a swap.
        loop {
            let moving: HashSet<PathBuf> =
                plans.iter().map(|(s, _)| fs_model::normalize(s)).collect();
            let before = plans.len();
            let mut kept: Vec<(PathBuf, PathBuf)> = Vec::new();
            for (src, target) in plans.drain(..) {
                if target.exists() && !moving.contains(&fs_model::normalize(&target)) {
                    problems.push(format!(
                        "{}: {} already exists",
                        display_name(&src),
                        display_name(&target)
                    ));
                    continue;
                }
                kept.push((src, target));
            }
            plans = kept;
            if plans.len() == before {
                break;
            }
        }
        if plans.is_empty() {
            if problems.is_empty() {
                self.toast("Nothing to rename".into());
            } else {
                self.toast_err(format!("Rename failed: {}", problems.join("; ")));
            }
            return;
        }

        // Phase one: out of the way. The temporary name carries the process id
        // and the item's place so no two of them are alike, and `unique_dest`
        // guards even against a real file already under that name.
        let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
        for (i, (src, target)) in plans.iter().enumerate() {
            let Some(parent) = src.parent() else { continue };
            let tmp = fs_model::unique_dest(
                &parent.join(format!(".rhumb-rename-{}-{i}", std::process::id())),
            );
            if let Err(e) = std::fs::rename(src, &tmp) {
                problems.push(format!("{}: {e}", display_name(src)));
                // Whatever already moved goes back, so a failure leaves nothing
                // sitting under a temporary name.
                for (moved, _, back) in staged.iter().rev() {
                    let _ = std::fs::rename(moved, back);
                }
                self.toast_err(format!("Rename failed: {}", problems.join("; ")));
                return;
            }
            staged.push((tmp, target.clone(), src.clone()));
        }

        // Phase two: to the final name. A failure here puts the rest of the
        // staged items back too, for the same reason.
        let mut renamed: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (i, (tmp, target, original)) in staged.iter().enumerate() {
            match std::fs::rename(tmp, target) {
                Ok(()) => {
                    if let Some(doc) = self.doc_mut()
                        && doc.path == *original
                    {
                        doc.path = target.clone();
                    }
                    renamed.push((original.clone(), target.clone()));
                }
                Err(e) => {
                    problems.push(format!("{}: {e}", display_name(original)));
                    let _ = std::fs::rename(tmp, original);
                    for (rest, _, back) in staged.iter().skip(i + 1).rev() {
                        let _ = std::fs::rename(rest, back);
                    }
                    break;
                }
            }
        }
        if renamed.is_empty() {
            self.toast_err(format!("Rename failed: {}", problems.join("; ")));
            return;
        }
        let undo = Undo::RenamedBatch(renamed.clone());
        self.undo_stack.push(undo.clone());
        self.undo = Some(undo);
        // The renamed items are the selection now, and the listing catches up.
        self.sel = renamed.iter().map(|(_, to)| to.clone()).collect();
        self.request_listing();
        if problems.is_empty() {
            self.toast(format!("Renamed {} item(s)", renamed.len()));
        } else {
            self.toast_err(format!(
                "Renamed {} item(s); skipped {}",
                renamed.len(),
                problems.join("; ")
            ));
        }
    }

    /// Makes a symbolic link beside `path`, named `<name> - link`, pointing at
    /// it. On Windows this needs Developer Mode or an elevated window; a refusal
    /// is reported as a toast, never a panic.
    pub(super) fn create_symlink(&mut self, path: &Path) {
        if self.refuse_in_archive(&[path]) || crate::recycle::is_item(path) {
            return;
        }
        let Some(parent) = path.parent() else { return };
        let Some(name) = path.file_name() else { return };
        let target =
            fs_model::unique_dest(&parent.join(format!("{} - link", name.to_string_lossy())));
        let is_dir = path.is_dir();
        #[cfg(windows)]
        let made = if is_dir {
            std::os::windows::fs::symlink_dir(path, &target)
        } else {
            std::os::windows::fs::symlink_file(path, &target)
        };
        #[cfg(not(windows))]
        let made = {
            let _ = is_dir;
            std::os::unix::fs::symlink(path, &target)
        };
        match made {
            Ok(()) => {
                self.toast(format!("Created link {}", display_name(&target)));
                self.request_listing();
            }
            Err(e) => self.toast_err(format!(
                "Could not create the link: {e}. On Windows a symbolic link needs Developer Mode or an elevated window."
            )),
        }
    }

    /// Makes a directory junction beside a folder, named `<name> - link`.
    /// Junctions are a Windows thing, and `mklink /J` normally needs no
    /// privilege, so this is the link that works on a locked-down machine.
    #[cfg(windows)]
    pub(super) fn create_junction(&mut self, path: &Path) {
        if self.refuse_in_archive(&[path]) || crate::recycle::is_item(path) {
            return;
        }
        if !path.is_dir() {
            self.toast_err("A junction can only point at a folder".into());
            return;
        }
        let Some(parent) = path.parent() else { return };
        let Some(name) = path.file_name() else { return };
        let target =
            fs_model::unique_dest(&parent.join(format!("{} - link", name.to_string_lossy())));
        // `mklink` is a `cmd` builtin, not an executable, so it has to run
        // inside a shell. The output is captured so a refusal can be shown.
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&target)
            .arg(path)
            .output();
        match made {
            Ok(out) if out.status.success() => {
                self.toast(format!("Created junction {}", display_name(&target)));
                self.request_listing();
            }
            Ok(out) => {
                let msg = String::from_utf8_lossy(&out.stderr).trim().to_owned();
                let msg = if msg.is_empty() {
                    "mklink refused".to_owned()
                } else {
                    msg
                };
                self.toast_err(format!("Could not create the junction: {msg}"));
            }
            Err(e) => self.toast_err(format!("Could not create the junction: {e}")),
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
                let undo = Undo::Created(vec![target.clone()]);
                self.undo_stack.push(undo.clone());
                self.undo = Some(undo);
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
            Undo::RenamedBatch(items) => {
                // Backwards, so a swap unwinds without the second move landing
                // on the first: the last item returns first.
                for (from, to) in items.iter().rev() {
                    if let Err(e) = std::fs::rename(to, from) {
                        problems.push(format!("{}: {e}", display_name(to)));
                    } else if let Some(doc) = self.doc_mut()
                        && doc.path == *to
                    {
                        doc.path = from.clone();
                    }
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
