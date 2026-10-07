//! The modal dialogs.

use super::*;

impl Rhumb {
    pub(super) fn dialogs(&mut self, ctx: &Context) {
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let next = match dialog {
            Dialog::None => None,
            Dialog::Rename { path, name } => {
                let mut name = name;
                let mut ok = false;
                let mut cancel = false;
                self.field_dialog(
                    ctx,
                    "rename_dlg",
                    "Rename",
                    &mut name,
                    "Rename",
                    "Cancel",
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    self.apply_rename(&path, &name);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::Rename { path, name })
                }
            }
            Dialog::BatchRename {
                paths,
                pattern,
                start,
            } => {
                let mut pattern = pattern;
                let mut start = start;
                let mut ok = false;
                let mut cancel = false;
                self.batch_rename_dialog(
                    ctx,
                    &paths,
                    &mut pattern,
                    &mut start,
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    self.apply_batch_rename(&paths, &pattern, &start);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::BatchRename {
                        paths,
                        pattern,
                        start,
                    })
                }
            }
            Dialog::Create { dir, name, folder } => {
                let mut name = name;
                let mut ok = false;
                let mut cancel = false;
                self.field_dialog(
                    ctx,
                    "create_dlg",
                    if folder { "New folder" } else { "New file" },
                    &mut name,
                    "Create",
                    "Cancel",
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    self.apply_create(&dir, &name, folder);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::Create { dir, name, folder })
                }
            }
            Dialog::Path { text } => {
                let mut text = text;
                let mut ok = false;
                let mut cancel = false;
                self.field_dialog(
                    ctx,
                    "path_dlg",
                    "Go to folder",
                    &mut text,
                    "Go",
                    "Cancel",
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    match fs_model::resolve_input(&text, &self.cwd) {
                        Some(p) if p.is_dir() => {
                            self.navigate(&p);
                            None
                        }
                        Some(p) => {
                            self.toast_err(format!("{} is not a folder", p.display()));
                            Some(Dialog::Path { text })
                        }
                        None => Some(Dialog::Path { text }),
                    }
                } else if cancel {
                    None
                } else {
                    Some(Dialog::Path { text })
                }
            }
            Dialog::ConfirmDelete { paths } => {
                let mut ok = false;
                let mut cancel = false;
                Modal::new(Id::new("confirm_delete"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        let what = if paths.len() == 1 {
                            format!("\u{201C}{}\u{201D}", display_name(&paths[0]))
                        } else {
                            format!("{} items", paths.len())
                        };
                        ui.label(format!("Permanently delete {what}?"));
                        ui.label(
                            egui::RichText::new("This cannot be undone.")
                                .color(c::TEXT_FAINT)
                                .size(theme::fs::SMALL),
                        );
                        ui.add_space(sp::MD);
                        ui.horizontal(|ui| {
                            if ui.button("Delete permanently").clicked() {
                                ok = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if ok {
                    self.start_permanent_delete(paths);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::ConfirmDelete { paths })
                }
            }
            Dialog::Collision {
                mut ready,
                mut conflicts,
                dest_dir,
                cut,
                mut apply_all,
            } => {
                let src = conflicts[0].clone();
                let name = display_name(&src);
                let folder = src.is_dir();
                let mut choice: Option<ConflictChoice> = None;
                let mut cancel = false;
                Modal::new(Id::new("collision"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(430.0);
                        ui.label(format!(
                            "\u{201C}{name}\u{201D} already exists in {}",
                            dest_dir.display()
                        ));
                        ui.label(
                            egui::RichText::new(if folder {
                                "Replace merges into the folder that is there; Keep both puts a copy beside it."
                            } else {
                                "Replace overwrites the file that is there; Keep both puts a copy beside it."
                            })
                            .color(c::TEXT_FAINT)
                            .size(theme::fs::SMALL),
                        );
                        ui.add_space(sp::MD);
                        if conflicts.len() > 1 {
                            ui.checkbox(
                                &mut apply_all,
                                format!("Do this for all {} remaining", conflicts.len()),
                            );
                            ui.add_space(sp::XS);
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Replace").clicked() {
                                choice = Some(ConflictChoice::Replace);
                            }
                            if ui.button("Skip").clicked() {
                                choice = Some(ConflictChoice::Skip);
                            }
                            if ui.button("Keep both").clicked() {
                                choice = Some(ConflictChoice::KeepBoth);
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if cancel {
                    self.toast("Transfer cancelled".into());
                    None
                } else if let Some(choice) = choice {
                    if apply_all {
                        for src in conflicts.drain(..) {
                            resolve_conflict(&mut ready, &dest_dir, src, choice);
                        }
                        self.finish_transfer(ready, cut);
                        None
                    } else {
                        resolve_conflict(&mut ready, &dest_dir, conflicts.remove(0), choice);
                        if conflicts.is_empty() {
                            self.finish_transfer(ready, cut);
                            None
                        } else {
                            Some(Dialog::Collision {
                                ready,
                                conflicts,
                                dest_dir,
                                cut,
                                apply_all: false,
                            })
                        }
                    }
                } else {
                    Some(Dialog::Collision {
                        ready,
                        conflicts,
                        dest_dir,
                        cut,
                        apply_all,
                    })
                }
            }
            Dialog::Unsaved { path, close_app } => {
                let mut save = false;
                let mut discard = false;
                let mut cancel = false;
                Modal::new(Id::new("unsaved"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        ui.label(format!(
                            "Save changes to \u{201C}{}\u{201D}?",
                            display_name(&path)
                        ));
                        ui.add_space(sp::MD);
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                save = true;
                            }
                            if ui.button("Discard").clicked() {
                                discard = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if save || discard {
                    if save {
                        self.save_doc();
                    }
                    // The prompt came from a tab cross, or from Ctrl+W on the
                    // active tab; either way close the tab it named.
                    match self.pending_close.take() {
                        Some(i) if i < self.tabs.len() => self.close_tab(i),
                        _ => self.close_active_tab(),
                    }
                    if close_app {
                        self.close_armed = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    None
                } else if cancel {
                    self.pending_close = None;
                    None
                } else {
                    Some(Dialog::Unsaved { path, close_app })
                }
            }
            Dialog::Properties { path } => {
                let mut close = false;
                Modal::new(Id::new("properties"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        properties_ui(ui, &path, self.measures.get(&path));
                        ui.add_space(sp::MD);
                        if ui.button("OK").clicked() {
                            close = true;
                        }
                    });
                if !close {
                    Some(Dialog::Properties { path })
                } else {
                    None
                }
            }
            Dialog::Settings { section } => {
                let mut section = section;
                let mut close = false;
                let modal = Modal::new(Id::new("settings"))
                    .frame(theme::dialog_frame().inner_margin(Margin::ZERO))
                    .show(ctx, |ui| self.settings_ui(ui, &mut section, &mut close));
                if modal.should_close() {
                    close = true;
                }
                if close {
                    None
                } else {
                    Some(Dialog::Settings { section })
                }
            }
            Dialog::Help => {
                let mut close = false;
                Modal::new(Id::new("help"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(460.0);
                        ui.label(egui::RichText::new("Shortcuts").strong());
                        ui.add_space(sp::SM);
                        let rows: &[(&str, &str)] = &[
                            ("Enter  /  double-click", "Open"),
                            ("Backspace  /  Alt+Up", "Up one folder"),
                            ("Alt+Left  /  Alt+Right", "Back  /  forward"),
                            ("Ctrl+L", "Go to folder"),
                            ("Ctrl+F", "Focus the search box"),
                            ("Ctrl+1  /  2  /  3", "Details  /  list  /  large icons"),
                            ("Ctrl+Shift+V", "Cycle the view"),
                            ("Ctrl+B", "Toggle the sidebar"),
                            ("Alt+P", "Toggle the details pane"),
                            ("Alt+Enter", "Properties"),
                            ("Ctrl+H", "Show hidden items"),
                            ("F5", "Reload the folder"),
                            ("Ctrl+A", "Select all"),
                            ("Space", "Toggle the row under the cursor"),
                            ("Shift+Up  /  Shift+Down", "Extend selection"),
                            ("Ctrl+C  /  Ctrl+X  /  Ctrl+V", "Copy  /  cut  /  paste"),
                            ("Delete", "Move to trash"),
                            ("Shift+Delete", "Delete permanently"),
                            ("Ctrl+Z", "Undo the last file operation"),
                            ("F2", "Rename"),
                            ("Ctrl+N  /  Ctrl+Shift+N", "New document  /  folder"),
                            ("Alt+F", "The New menu, including compress to ZIP"),
                            ("Ctrl+S  /  Ctrl+W", "Save  /  close the tab"),
                            (
                                "Ctrl+D",
                                "In the editor: select the word, then the next match",
                            ),
                            ("Ctrl+Shift+L", "In the editor: a caret on every match"),
                            (
                                "Ctrl+Alt+Up  /  Down",
                                "In the editor: add a caret above  /  below",
                            ),
                            ("Alt+click", "In the editor: add or remove a caret"),
                            ("Esc", "In the editor: back to one caret"),
                            ("Ctrl+Shift+D", "In the editor: duplicate the line"),
                            ("Ctrl+T", "New tab showing files"),
                            ("Ctrl+Tab  /  Ctrl+Shift+Tab", "Next  /  previous tab"),
                            ("Tab  /  Shift+Tab", "Indent  /  outdent"),
                            ("Ctrl+/", "Toggle a line comment"),
                            ("Escape", "Clear the search, then the selection"),
                            ("?  /  /", "This list"),
                        ];
                        egui::Grid::new("help_grid")
                            .num_columns(2)
                            .spacing([sp::LG, sp::XS])
                            .show(ui, |ui| {
                                for (k, v) in rows {
                                    ui.label(egui::RichText::new(*k).monospace().color(c::TEXT));
                                    ui.label(
                                        egui::RichText::new(*v)
                                            .color(c::TEXT_FAINT)
                                            .size(theme::fs::SMALL),
                                    );
                                    ui.end_row();
                                }
                            });
                        ui.add_space(sp::MD);
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                if close { None } else { Some(Dialog::Help) }
            }
        };
        self.dialog = next.unwrap_or(Dialog::None);
    }

    /// A single-field dialog, with live validation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn field_dialog(
        &mut self,
        ctx: &Context,
        id: &str,
        title: &str,
        value: &mut String,
        confirm: &str,
        cancel: &str,
        ok: &mut bool,
        cancelled: &mut bool,
    ) {
        Modal::new(Id::new(id))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(egui::RichText::new(title).strong());
                ui.add_space(sp::SM);
                let mut typed = false;
                let out = TextEdit::singleline(value)
                    .id(Id::new((id, "field")))
                    .desired_width(f32::INFINITY)
                    .frame(
                        Frame::new()
                            .fill(c::CODE_BG)
                            .stroke(Stroke::new(1.0, c::BORDER))
                            .corner_radius(CornerRadius::same(sp::RADIUS))
                            .inner_margin(Margin::symmetric(sp::SM_I, 6)),
                    )
                    .show(ui);
                typed |= out.response.changed();

                let check_name = !id.contains("path");
                let validation = if check_name {
                    fs_model::validate_name(value)
                } else {
                    Ok(())
                };
                if let Err(msg) = &validation {
                    ui.label(
                        egui::RichText::new(msg)
                            .color(c::DANGER)
                            .size(theme::fs::SMALL),
                    );
                }
                let enabled = validation.is_ok() && !value.trim().is_empty();
                ui.add_space(sp::MD);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(enabled, egui::Button::new(confirm))
                        .clicked()
                    {
                        *ok = true;
                    }
                    if ui.button(cancel).clicked() {
                        *cancelled = true;
                    }
                });
                // Keep focus in the field as the dialog persists across frames.
                if typed {
                    out.response.request_focus();
                }
            });
    }

    /// The batch-rename dialog: a pattern, a start number and a live preview of
    /// the first few `old -> new` pairs, so the result is seen before it happens.
    pub(super) fn batch_rename_dialog(
        &mut self,
        ctx: &Context,
        paths: &[PathBuf],
        pattern: &mut String,
        start: &mut String,
        ok: &mut bool,
        cancelled: &mut bool,
    ) {
        let start_n: usize = start.trim().parse().unwrap_or(1);
        Modal::new(Id::new("batch_rename"))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(egui::RichText::new(format!("Rename {} items", paths.len())).strong());
                ui.add_space(sp::SM);

                let field = |ui: &mut Ui, value: &mut String, id: &str, width: f32| {
                    let out = TextEdit::singleline(value)
                        .id(Id::new(id))
                        .desired_width(width)
                        .frame(
                            Frame::new()
                                .fill(c::CODE_BG)
                                .stroke(Stroke::new(1.0, c::BORDER))
                                .corner_radius(CornerRadius::same(sp::RADIUS))
                                .inner_margin(Margin::symmetric(sp::SM_I, 6)),
                        )
                        .show(ui);
                    // Keep the caret in whichever field is being typed in as
                    // the dialog rebuilds itself every frame.
                    if out.response.changed() {
                        out.response.request_focus();
                    }
                };

                ui.label(
                    egui::RichText::new("Pattern")
                        .color(c::TEXT_FAINT)
                        .size(theme::fs::SMALL),
                );
                field(ui, pattern, "batch_rename_pattern", f32::INFINITY);

                ui.add_space(sp::XS);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Start at")
                            .color(c::TEXT_FAINT)
                            .size(theme::fs::SMALL),
                    );
                    field(ui, start, "batch_rename_start", 64.0);
                });
                ui.add_space(sp::XS);
                ui.label(
                    egui::RichText::new(
                        "Tokens: {name} the original name, {ext} the extension, \
                         {n} a counter. {n:3} pads the counter to three digits.",
                    )
                    .color(c::TEXT_FAINT)
                    .size(theme::fs::SMALL),
                );

                ui.add_space(sp::MD);
                ui.label(
                    egui::RichText::new("Preview")
                        .color(c::TEXT_FAINT)
                        .size(theme::fs::SMALL),
                );
                // Only a handful of pairs are drawn; a folder of thousands is
                // still one short list in the dialog.
                let mut all_valid = true;
                for (i, p) in paths.iter().take(8).enumerate() {
                    let stem = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let ext = p
                        .extension()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let new = fs_model::batch_name(pattern, &stem, &ext, start_n + i);
                    let bad = fs_model::validate_name(new.trim()).is_err();
                    all_valid &= !bad;
                    ui.label(
                        egui::RichText::new(format!("{}  \u{2192}  {}", display_name(p), new))
                            .color(if bad { c::DANGER } else { c::TEXT_DIM })
                            .size(theme::fs::SMALL),
                    );
                }
                if paths.len() > 8 {
                    ui.label(
                        egui::RichText::new(format!("\u{2026} and {} more", paths.len() - 8))
                            .color(c::TEXT_FAINT)
                            .size(theme::fs::SMALL),
                    );
                }

                ui.add_space(sp::MD);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(all_valid, egui::Button::new("Rename"))
                        .clicked()
                    {
                        *ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        *cancelled = true;
                    }
                });
            });
    }
}

/// Puts one resolved conflict into the list of pairs to transfer: the destination
/// name it keeps is the plain name for replace, and a fresh one for "keep both".
fn resolve_conflict(
    ready: &mut Vec<(PathBuf, PathBuf)>,
    dest_dir: &Path,
    src: PathBuf,
    choice: ConflictChoice,
) {
    let Some(name) = src.file_name() else {
        return;
    };
    let target = dest_dir.join(name);
    match choice {
        ConflictChoice::Replace => ready.push((src, target)),
        ConflictChoice::Skip => {}
        ConflictChoice::KeepBoth => ready.push((src, fs_model::unique_dest(&target))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn each_conflict_choice_picks_the_right_destination() {
        let root = std::env::temp_dir().join(format!("rhumb-conflict-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (dst, other) = (root.join("dst"), root.join("other"));
        fs::create_dir_all(&dst).unwrap();
        fs::create_dir_all(&other).unwrap();
        // The name is taken at the destination.
        fs::write(dst.join("a.txt"), b"old").unwrap();
        let src = other.join("a.txt");
        fs::write(&src, b"new").unwrap();

        let mut ready = Vec::new();
        resolve_conflict(&mut ready, &dst, src.clone(), ConflictChoice::Replace);
        assert_eq!(ready, vec![(src.clone(), dst.join("a.txt"))]);

        ready.clear();
        resolve_conflict(&mut ready, &dst, src.clone(), ConflictChoice::Skip);
        assert!(ready.is_empty(), "skip places nothing");

        ready.clear();
        resolve_conflict(&mut ready, &dst, src.clone(), ConflictChoice::KeepBoth);
        assert_eq!(ready, vec![(src, dst.join("a (2).txt"))]);
        let _ = fs::remove_dir_all(&root);
    }
}
