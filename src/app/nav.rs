//! Moving between folders, the folder listing and the messages from workers.

use super::*;

impl Rhumb {
    /// Folds in anything a second launch handed this window.
    ///
    /// Checked on the slow tick rather than every frame: it is a file
    /// existence test, and launches are rare. Either opens the path or just
    /// brings the window forward, either way the window ends up focused.
    pub(super) fn take_instance_signal(&mut self, ctx: &Context) {
        let Some(request) = crate::instance::take_signal() else {
            return;
        };
        match request {
            Some(p) if p.is_file() => {
                if let Some(parent) = p.parent() {
                    self.navigate(parent);
                }
                self.open_path(&p);
            }
            Some(p) if p.is_dir() => self.navigate(&p),
            // No path: the user just wants the window they already have.
            _ => {}
        }
        // Raise it if it was minimised, then focus it.
        if ctx.input(|i| i.viewport().minimized.unwrap_or(false)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Applies everything the workers have finished. Nothing here touches the
    /// disk, so a busy copy or search never stalls a frame.
    pub(super) fn drain_messages(&mut self, ctx: &Context) {
        for msg in workers::drain(&self.rx) {
            match msg {
                Msg::Listed {
                    token,
                    path,
                    entries,
                    error,
                } => {
                    if token != self.req || path != self.cwd {
                        continue;
                    }
                    match error {
                        Some(e) => {
                            log::warn!("listing failed: {e}");
                            self.entries.clear();
                            self.visible.clear();
                            self.listing = Listing::Failed;
                        }
                        None => {
                            self.entries = entries;
                            self.listed_at = Some(Instant::now());
                            fs_model::sort(&mut self.entries, self.sort, self.ascending);
                            self.listing = Listing::Ready;
                            self.row_cache.clear();
                            self.thumbs.clear();
                            self.recompute_visible();
                            if let Some(a) = self.restore_anchor.take() {
                                self.anchor = a.min(self.visible.len().saturating_sub(1));
                            }
                        }
                    }
                    let cwd = self.cwd.clone();
                    self.watch(&cwd);
                }
                Msg::Loaded {
                    path, doc, error, ..
                } => {
                    if self.loading.as_deref() != Some(path.as_path()) {
                        continue;
                    }
                    self.loading = None;
                    match (error, doc) {
                        (Some(e), _) => self.toast_err(e),
                        (None, Some(doc)) => {
                            // Opening is never refused, so say why the buffer
                            // is not editable when the file was not text.
                            if doc.looks_binary {
                                self.toast(format!(
                                    "{} does not look like text; opened read-only",
                                    doc.path.file_name().map_or_else(
                                        || doc.path.to_string_lossy().into_owned(),
                                        |n| n.to_string_lossy().into_owned(),
                                    )
                                ));
                            } else if doc.too_large {
                                self.toast(format!(
                                    "{} is too large to edit here",
                                    doc.path.file_name().map_or_else(
                                        || doc.path.to_string_lossy().into_owned(),
                                        |n| n.to_string_lossy().into_owned(),
                                    )
                                ));
                            }
                            // The tab was created when the read started, so the
                            // document goes into the tab already on screen.
                            match self.tab_index(&path) {
                                Some(i) => {
                                    self.tabs.focus(i);
                                    if let Some(t) = self.tabs.get_mut(i) {
                                        t.doc = doc;
                                    }
                                }
                                None => {
                                    // The tab it was opened in may have been left while it
                                    // loaded, and the file belongs there, not here.
                                    let parked = self.folders.iter_mut().find_map(|f| {
                                        f.parked
                                            .as_mut()
                                            .and_then(|p| p.docs.index_of(&path).map(|i| (p, i)))
                                    });
                                    if let Some((p, i)) = parked {
                                        p.docs[i].doc = doc;
                                    } else {
                                        let i = self.tabs.push(Tab {
                                            id: next_tab_id(),
                                            doc,
                                        });
                                        self.tabs.active = i;
                                    }
                                }
                            }
                            self.preview.reset();
                            self.render_version = 1;
                            self.preview_buffer_version = u64::MAX;
                            self.preview.reset();
                        }
                        (None, None) => {}
                    }
                }
                Msg::Progress(p) => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.job.id == p.id) {
                        j.done_items = p.done_items;
                        j.total_items = p.total_items;
                        j.done_bytes = p.done_bytes;
                        j.total_bytes = p.total_bytes;
                        j.current = p.current;
                        if !p.failed.is_empty() {
                            self.toast_err(p.failed[0].clone());
                        }
                    }
                }
                Msg::Finished { id, outcome } => {
                    self.jobs.retain(|j| j.job.id != id);
                    self.settle_pending_undo();
                    // The job changed what is on disk, so cached subtree
                    // totals for the folders it touched are no longer true.
                    self.measures.clear();
                    // And so are the names an index holds for the folder being shown.
                    let shown = self.cwd.clone();
                    self.indexes.changed(&shown);
                    if let Outcome::Failed(msg) = &outcome {
                        self.toast_err(msg.clone());
                    }
                    self.request_listing();
                    match outcome {
                        Outcome::Done { ok, failed } => {
                            if failed.is_empty() {
                                self.toast(format!("Done \u{00B7} {ok} item(s)"));
                            } else {
                                self.toast_err(format!("{ok} done, {} failed", failed.len()));
                                for f in failed.iter().take(3) {
                                    log::warn!("op failed: {f}");
                                }
                            }
                        }
                        Outcome::Cancelled { done } => {
                            self.toast(format!("Cancelled after {done} item(s)"));
                        }
                        Outcome::Failed(e) => self.toast_err(e),
                    }
                }
                Msg::Search(chunk) => {
                    // The answer to a search that has since been replaced.
                    if chunk.token != self.search.token {
                        continue;
                    }
                    self.search.results.extend(chunk.found);
                    self.search.scanned = chunk.scanned;
                    self.search.truncated = chunk.truncated;
                    if chunk.done {
                        self.search.running = false;
                        self.search_shown = true;
                        self.row_cache.clear();
                    }
                }
                Msg::Peek { path, text } => {
                    if self.peek_pending.as_deref() == Some(path.as_path()) {
                        self.peek_pending = None;
                    }
                    if let Some(text) = text {
                        self.peek = Some((path, text));
                    }
                }
                Msg::Thumb {
                    path,
                    px,
                    rgba,
                    w,
                    h,
                } => {
                    self.thumbs.insert(path, px, rgba, w, h, ctx);
                }
                Msg::Measured { path, measure } => {
                    self.measures.set(path, measure);
                }
                Msg::TreeLoaded { path, dirs } => {
                    if self.sidebar_tree.loading.as_deref() == Some(path.as_path()) {
                        self.sidebar_tree.loading = None;
                    }
                    self.sidebar_tree.tree.set_children(&path, dirs);
                }
                Msg::Watch(path) => {
                    self.last_change = Some(Instant::now());
                    // The names in an index follow what changed on disk.
                    if let Some(dir) = path.parent() {
                        self.indexes.changed(dir);
                    }
                    if let Some(doc) = self.doc_mut()
                        && doc.path == path
                    {
                        doc.check_external_change();
                    }
                }
            }
        }
    }

    pub(super) fn handle_watch_debounce(&mut self) {
        let Some(last) = self.last_change else { return };
        if last.elapsed() < WATCH_DEBOUNCE {
            return;
        }
        self.last_change = None;
        // Reading a folder can itself raise a change event, so a refresh can
        // trigger the next one. Ignoring events for a moment after each listing
        // breaks that loop; a real edit still shows up, just not instantly.
        if let Some(listed) = self.listed_at
            && listed.elapsed() < WATCH_COOLDOWN
        {
            return;
        }
        self.request_listing();
        if let Some(doc) = self.doc() {
            let dir = doc.path.parent().map(|p| p.to_path_buf());
            if let Some(dir) = dir {
                self.watch(&dir);
            }
        }
    }

    /// Starts indexing the folder being shown, unless an index already covers it, so
    /// that by the time anything is typed in the search box the names are there. A
    /// drive's own root is left alone until it is searched: indexing a whole drive is
    /// not something to do because it was looked at.
    pub(super) fn start_index_for_cwd(&mut self) {
        let cwd = self.cwd.clone();
        if cwd.parent().is_none() || !cwd.is_dir() || self.indexes.any_for(&cwd).is_some() {
            return;
        }
        self.indexes.ensure(&cwd);
    }

    pub(super) fn navigate(&mut self, path: &Path) {
        let path = fs_model::normalize(path);
        if path == self.cwd {
            return;
        }
        self.history.push(self.cwd.clone());
        self.cwd = path.clone();
        self.start_index_for_cwd();
        // Show where we landed in the tree.
        self.sidebar_tree.tree.reveal(&path);
        self.after_jump();
    }

    pub(super) fn go_back(&mut self) {
        if let Some(prev) = self.history.back(&self.cwd) {
            self.cwd = fs_model::normalize(&prev);
            self.after_jump();
        }
    }

    pub(super) fn go_forward(&mut self) {
        if let Some(next) = self.history.forward(&self.cwd) {
            self.cwd = fs_model::normalize(&next);
            self.after_jump();
        }
    }

    /// Shared reset after the folder changes.
    pub(super) fn after_jump(&mut self) {
        self.sync_folder_tab();
        self.sel.clear();
        self.typeahead.clear();
        self.cursor = 0;
        self.anchor = 0;
        self.filter.clear();
        self.search_typed = None;
        self.clear_search();
        self.row_cache.clear();
        self.listing = Listing::Loading;
        self.request_listing();
    }

    pub(super) fn go_up(&mut self) {
        if self.searching() {
            self.clear_search();
            self.recompute_visible();
            return;
        }
        let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        if parent == self.cwd {
            return;
        }
        let came_from = self
            .cwd
            .file_name()
            .map(|s| s.to_string_lossy().to_string());
        self.navigate(&parent);
        if let Some(name) = came_from
            && let Some(i) = self
                .visible
                .iter()
                .position(|e| self.entries.get(*e).is_some_and(|e| e.name == name))
        {
            self.cursor = i;
            self.anchor = i;
            if let Some(e) = self.entries.get(self.visible[i]) {
                self.sel.insert(e.path.clone());
            }
        }
    }

    /// Re-reads the current folder on a worker thread.
    pub(super) fn request_listing(&mut self) {
        self.req = self.ids.next();
        let token = self.req;
        let path = self.cwd.clone();
        let show_hidden = self.show_hidden;
        let tx = self.tx.clone();
        self.listing = Listing::Loading;
        self.row_cache.clear();
        let _ = std::thread::Builder::new()
            .name("rhumb-list".into())
            .spawn(move || {
                let (entries, error) = match fs_model::read_dir(&path, show_hidden) {
                    Ok(e) => (e, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                let _ = tx.send(Msg::Listed {
                    token,
                    path,
                    entries,
                    error,
                });
            });
    }

    /// Watches one folder for changes, so external edits show up on their own.
    pub(super) fn watch(&mut self, path: &Path) {
        // Nothing on the disk to watch inside an archive; the file itself is in the
        // folder above, which is watched when that is shown.
        if archive::is_virtual(path) {
            return;
        }
        if self.watch_target.as_deref() == Some(path) {
            return;
        }
        self.watcher = None;
        self.watch_target = None;
        let tx = self.tx.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res
                    && let Some(p) = ev.paths.first()
                {
                    let _ = tx.send(Msg::Watch(p.clone()));
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("watcher unavailable: {e}");
                    return;
                }
            };
        if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
            log::warn!("cannot watch {}: {e}", path.display());
            return;
        }
        self.watcher = Some(Box::new(watcher));
        self.watch_target = Some(path.to_path_buf());
    }

    // ---- toasts -------------------------------------------------------------------
}
