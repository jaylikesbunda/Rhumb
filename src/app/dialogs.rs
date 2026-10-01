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
}
