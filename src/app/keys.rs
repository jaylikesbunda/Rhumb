//! The keyboard shortcuts of the window.

use super::*;

impl Rhumb {
    pub(super) fn handle_keys(&mut self, ctx: &Context) {
        let mut k = Keys::default();
        ctx.input_mut(|i| {
            k.help = i.consume_key(egui::Modifiers::NONE, Key::Slash);
            k.alt_left = i.consume_key(egui::Modifiers::ALT, Key::ArrowLeft);
            k.alt_right = i.consume_key(egui::Modifiers::ALT, Key::ArrowRight);
            k.alt_up = i.consume_key(egui::Modifiers::ALT, Key::ArrowUp);
            k.go_path = i.consume_key(egui::Modifiers::CTRL, Key::L)
                || i.consume_key(egui::Modifiers::COMMAND, Key::L);
            k.focus_search = i.consume_key(egui::Modifiers::CTRL, Key::F)
                || i.consume_key(egui::Modifiers::COMMAND, Key::F);
            k.refresh = i.consume_key(egui::Modifiers::NONE, Key::F5);
            k.toggle_hidden = i.consume_key(egui::Modifiers::CTRL, Key::H)
                || i.consume_key(egui::Modifiers::COMMAND, Key::H);
            k.toggle_sidebar = i.consume_key(egui::Modifiers::CTRL, Key::B)
                || i.consume_key(egui::Modifiers::COMMAND, Key::B);
            k.close_file = i.consume_key(egui::Modifiers::CTRL, Key::W)
                || i.consume_key(egui::Modifiers::COMMAND, Key::W);
            k.new_tab = i.consume_key(egui::Modifiers::CTRL, Key::T)
                || i.consume_key(egui::Modifiers::COMMAND, Key::T);
            // The backward keys first: a press of Ctrl+Shift+Tab also matches Ctrl+Tab,
            // which takes any extra Shift, so asking for the plain one first made
            // Ctrl+Shift+Tab go forward.
            k.prev_tab = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Tab)
                || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::PageUp)
                || i.consume_key(egui::Modifiers::CTRL, Key::PageUp);
            k.next_tab = i.consume_key(egui::Modifiers::CTRL, Key::Tab)
                || i.consume_key(egui::Modifiers::CTRL, Key::PageDown);
            k.new_file = i.consume_key(egui::Modifiers::CTRL, Key::N);
            k.new_folder = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::N);
            k.save = i.consume_key(egui::Modifiers::CTRL, Key::S)
                || i.consume_key(egui::Modifiers::COMMAND, Key::S);
            k.select_all = i.consume_key(egui::Modifiers::CTRL, Key::A)
                || i.consume_key(egui::Modifiers::COMMAND, Key::A);
            // Ctrl+C, Ctrl+X and Ctrl+V are deliberately *not* read here. The
            // window layer turns those three into Copy, Cut and Paste events
            // and drops the key press, so asking for the key finds nothing and
            // the shortcut silently did nothing. They are handled as events
            // below, where the editor gets first refusal.
            k.rename = i.consume_key(egui::Modifiers::NONE, Key::F2);
            k.delete_forever = i.consume_key(egui::Modifiers::SHIFT, Key::Delete);
            k.delete = i.consume_key(egui::Modifiers::NONE, Key::Delete);
            k.undo = i.consume_key(egui::Modifiers::CTRL, Key::Z)
                || i.consume_key(egui::Modifiers::COMMAND, Key::Z);
            k.properties = i.consume_key(egui::Modifiers::ALT, Key::Enter);
            k.new_menu = i.consume_key(egui::Modifiers::ALT, Key::F);
            k.toggle_details = i.consume_key(egui::Modifiers::ALT, Key::P);
            k.settings = i.consume_key(egui::Modifiers::CTRL, Key::Comma)
                || i.consume_key(egui::Modifiers::COMMAND, Key::Comma);
            k.escape = i.consume_key(egui::Modifiers::NONE, Key::Escape);
            k.focus_mode = i.consume_key(egui::Modifiers::NONE, Key::F11);
            // Ctrl+1..3 pick the list layout, the way Explorer does.
            for (digit, key) in [(1u8, Key::Num1), (2, Key::Num2), (3, Key::Num3)] {
                if i.consume_key(egui::Modifiers::CTRL, key)
                    || i.consume_key(egui::Modifiers::COMMAND, key)
                {
                    k.view = ViewMode::from_digit(digit);
                }
            }
            if i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::V)
                || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::V)
            {
                k.cycle_view = true;
            }
        });
        k.search_go = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));

        if k.settings {
            self.dialog = if matches!(self.dialog, Dialog::Settings { .. }) {
                Dialog::None
            } else {
                Dialog::Settings { section: 0 }
            };
        }
        if k.help {
            self.dialog = if matches!(self.dialog, Dialog::Help) {
                Dialog::None
            } else {
                Dialog::Help
            };
        }
        if k.alt_left {
            self.go_back();
        }
        if k.alt_right {
            self.go_forward();
        }
        if k.alt_up {
            self.go_up();
        }
        if k.go_path {
            self.dialog = Dialog::Path {
                text: self.cwd.to_string_lossy().to_string(),
            };
        }
        if k.focus_search {
            self.search_focus = true;
        }
        if k.refresh {
            self.request_listing();
        }
        if k.toggle_hidden {
            self.show_hidden = !self.show_hidden;
            self.request_listing();
        }
        if k.toggle_sidebar {
            self.sidebar = !self.sidebar;
        }
        if k.new_menu {
            // Consumed by the toolbar's New button, which owns the popup id.
            self.new_menu = true;
        }
        if k.toggle_details {
            self.details = !self.details;
        }
        if k.focus_mode && self.shows_file_tab() {
            self.focus = !self.focus;
        }
        if k.close_file {
            self.close_doc();
        }
        if k.new_tab {
            self.new_folder_tab();
        }
        if k.next_tab {
            self.cycle_tab(false);
        }
        if k.prev_tab {
            self.cycle_tab(true);
        }
        if k.new_file {
            self.dialog = Dialog::Create {
                dir: self.cwd.clone(),
                name: String::new(),
                folder: false,
            };
        }
        if k.new_folder {
            self.dialog = Dialog::Create {
                dir: self.cwd.clone(),
                name: String::new(),
                folder: true,
            };
        }
        if k.save {
            self.save_doc();
        }
        // The editor claims its own keys with `consume_key` while it draws,
        // which happens before this pass, so a key that still arrives here was
        // not wanted by the caret. That makes these keys mean the file list
        // while the list is active and the text while the caret is, without
        // either side having to know the other exists.
        if k.select_all && !self.ed_focused {
            self.select_all();
        }
        self.handle_clipboard_events(ctx);
        if k.rename
            && let Some(p) = self.cursor_path()
        {
            self.start_rename(&p);
        }
        if k.delete_forever {
            self.delete_selection(true);
        }
        if k.delete && !self.ed_focused {
            self.delete_selection(false);
        }
        // Undo is the caret's while the caret has the keyboard. Without this,
        // an editor with nothing left to undo would quietly undo the last file
        // operation instead, which is not what pressing Ctrl+Z means there.
        if k.undo && !self.ed_focused {
            self.undo();
        }
        if k.properties
            && let Some(p) = self.cursor_path()
        {
            self.dialog = Dialog::Properties { path: p };
        }
        if let Some(mode) = k.view {
            self.set_view(mode);
        }
        if k.cycle_view {
            let next = self.view.next();
            self.set_view(next);
        }
        if k.escape {
            if !self.filter.is_empty() {
                self.filter.clear();
                self.clear_search();
                self.recompute_visible();
            } else if !self.sel.is_empty() {
                self.sel.clear();
            } else if self.searching() {
                self.clear_search();
            }
        }
        if k.search_go && self.filter_focused(ctx) {
            // Typing already searches; Enter just refreshes it immediately.
            self.pump_search();
        }

        // List navigation, but only when nothing else wants the keyboard.
        if !ctx.egui_wants_keyboard_input() && matches!(self.dialog, Dialog::None) {
            self.list_keys(ctx);
        }
        // Files dropped from the OS.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.first().map(|f| f.path().to_path_buf()) {
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
