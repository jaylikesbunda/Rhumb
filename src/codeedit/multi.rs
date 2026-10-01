//! More than one caret.
//!
//! The editor has a *primary* selection (`anchor` and `caret`), which is the one the
//! window follows, and any number of *extra* ones beside it. Every command is written
//! for one caret, and runs for all of them by [`Editor::each`]: the primary is swapped
//! for each cursor in turn, the command is run as it always was, and the others are
//! carried over whatever it did to the text. That keeps one definition of what
//! Backspace, Enter, Tab and a bracket typed all mean, however many carets there are.

use super::*;

/// The most carets there can be.
pub(super) const MAX_CURSORS: usize = 10_000;

/// One selection: where it started, where the caret is, and where the caret was
/// horizontally when a run of Up and Down began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cursor {
    pub anchor: usize,
    pub caret: usize,
    pub goal: Option<(usize, f32)>,
}

impl Cursor {
    pub fn new(anchor: usize, caret: usize) -> Cursor {
        Cursor {
            anchor,
            caret,
            goal: None,
        }
    }

    pub fn lo(&self) -> usize {
        self.anchor.min(self.caret)
    }

    pub fn hi(&self) -> usize {
        self.anchor.max(self.caret)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }
}

/// Every selection at one moment, for undo to put back.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Snap {
    pub primary: (usize, usize),
    pub extra: Vec<(usize, usize)>,
}

/// The state of a command that is being run for every caret.
pub(super) struct Group {
    /// The selections as they were before any of it, which is what undo goes back to.
    pub snap: Snap,
    /// Whether the undo step has been opened yet. It opens on the first edit, once.
    pub started: bool,
}

/// Where a position goes when `removed` characters at `at` become `inserted`.
///
/// A position before the change stays; one after it moves by the difference; one inside
/// what was removed goes to where the replacement ends, which is the start if nothing
/// replaced it. Used to carry the other carets along while one of them edits.
pub(super) fn remap(p: usize, at: usize, removed: usize, inserted: usize) -> usize {
    if p < at {
        p
    } else if p >= at + removed {
        // At the very start of an insertion, the caret that was there stays before it.
        if p == at && removed == 0 {
            p
        } else {
            p - removed + inserted
        }
    } else {
        at + inserted
    }
}

/// Puts the cursors in order, clamps them to the document, and joins any that overlap
/// or touch, so no two ever stand on the same text. `primary` is kept pointing at the
/// primary's cursor through all of it. With `lines`, cursors that share a line are
/// joined as well, for the commands that act on whole lines.
pub(super) fn normalize(
    all: &mut Vec<Cursor>,
    primary: &mut usize,
    len: usize,
    lines: Option<&Buffer>,
) {
    let mut tagged: Vec<(Cursor, bool)> = all
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut c = *c;
            c.anchor = c.anchor.min(len);
            c.caret = c.caret.min(len);
            (c, i == *primary)
        })
        .collect();
    tagged.sort_by_key(|(c, _)| (c.lo(), c.hi()));
    let mut out: Vec<(Cursor, bool)> = Vec::with_capacity(tagged.len());
    for (c, is_primary) in tagged {
        if let Some((last, last_primary)) = out.last_mut() {
            let touches =
                c.lo() < last.hi() || (c.lo() == last.hi() && (c.is_empty() || last.is_empty()));
            let same_line = lines.is_some_and(|t| {
                t.line_of_char(c.lo().min(len)) <= t.line_of_char(last.hi().min(len))
            });
            if touches || same_line {
                // One selection now, over both, going the way the earlier one went.
                let forward = last.caret >= last.anchor;
                let (lo, hi) = (last.lo().min(c.lo()), last.hi().max(c.hi()));
                let keep_goal = if is_primary { c.goal } else { last.goal };
                *last = if forward || lo == hi {
                    Cursor {
                        anchor: lo,
                        caret: hi,
                        goal: keep_goal,
                    }
                } else {
                    Cursor {
                        anchor: hi,
                        caret: lo,
                        goal: keep_goal,
                    }
                };
                *last_primary |= is_primary;
                continue;
            }
        }
        out.push((c, is_primary));
    }
    *primary = out.iter().position(|(_, p)| *p).unwrap_or(0);
    *all = out.into_iter().map(|(c, _)| c).collect();
}

impl Editor {
    /// How many carets there are.
    pub fn cursor_count(&self) -> usize {
        1 + self.extra.len()
    }

    /// Every selection, in the order they stand in the text, as `(anchor, caret)`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cursors(&self) -> Vec<(usize, usize)> {
        let mut all = self.all_cursors();
        let mut p = 0;
        normalize(&mut all, &mut p, usize::MAX, None);
        all.iter().map(|c| (c.anchor, c.caret)).collect()
    }

    /// Sets every selection at once, the last one being the primary. For tests.
    #[cfg(test)]
    pub fn set_selections(&mut self, v: &[(usize, usize)]) {
        let mut all: Vec<Cursor> = v.iter().map(|&(a, c)| Cursor::new(a, c)).collect();
        let mut p = all.len().saturating_sub(1);
        normalize(&mut all, &mut p, usize::MAX, None);
        self.set_cursors(all, p);
    }

    /// The primary and the extra ones as one list, the primary first.
    pub(super) fn all_cursors(&self) -> Vec<Cursor> {
        let mut all = Vec::with_capacity(1 + self.extra.len());
        all.push(Cursor {
            anchor: self.anchor,
            caret: self.caret,
            goal: self.goal,
        });
        all.extend(self.extra.iter().copied());
        all
    }

    /// Takes a list of cursors back in, with the one at `primary` as the primary.
    fn set_cursors(&mut self, mut all: Vec<Cursor>, primary: usize) {
        let p = all.remove(primary.min(all.len().saturating_sub(1)));
        self.anchor = p.anchor;
        self.caret = p.caret;
        self.goal = p.goal;
        self.extra = all;
    }

    /// The selections as they are now, for undo.
    pub(super) fn snapshot(&self) -> Snap {
        if let Some(g) = &self.group {
            return g.snap.clone();
        }
        Snap {
            primary: (self.anchor, self.caret),
            extra: self.extra.iter().map(|c| (c.anchor, c.caret)).collect(),
        }
    }

    /// Puts the selections back as a snapshot had them.
    pub(super) fn restore(&mut self, snap: &Snap, len: usize) {
        self.anchor = snap.primary.0.min(len);
        self.caret = snap.primary.1.min(len);
        self.goal = None;
        self.extra = snap
            .extra
            .iter()
            .map(|&(a, c)| Cursor::new(a.min(len), c.min(len)))
            .collect();
    }

    /// Drops every caret but the primary.
    pub fn collapse_cursors(&mut self) {
        self.extra.clear();
    }

    /// Runs `f` once for every caret, as if each were the only one.
    ///
    /// With a single caret this is just a call. With more, they are taken from the
    /// last in the text to the first, each in turn standing in for the primary; the
    /// edits `f` makes are followed through the others, so one command may change the
    /// text under all of them and none is left pointing at what is not there. Carets
    /// that end up on the same text are joined. With `by_line`, carets on the same
    /// line are joined first, for commands that act on whole lines and would otherwise
    /// act on one twice.
    pub(super) fn each(
        &mut self,
        text: &mut Buffer,
        by_line: bool,
        mut f: impl FnMut(&mut Editor, &mut Buffer),
    ) {
        if self.extra.is_empty() {
            f(self, text);
            return;
        }
        let mut all = self.all_cursors();
        let mut prim = 0usize;
        let len = text.len_chars();
        normalize(&mut all, &mut prim, len, by_line.then_some(&*text));
        // What undo returns to: all of them, as they stood before the command.
        let snap = Snapshot::of(&all, prim);
        self.group = Some(Group {
            snap: snap.0,
            started: false,
        });
        self.extra.clear();
        for i in (0..all.len()).rev() {
            self.anchor = all[i].anchor;
            self.caret = all[i].caret;
            self.goal = all[i].goal;
            text.trace_begin();
            f(self, text);
            let edits = text.trace_take();
            all[i] = Cursor {
                anchor: self.anchor,
                caret: self.caret,
                goal: self.goal,
            };
            if !edits.is_empty() {
                for (j, c) in all.iter_mut().enumerate() {
                    if j == i {
                        continue;
                    }
                    for &(at, removed, inserted) in &edits {
                        c.anchor = remap(c.anchor, at, removed, inserted);
                        c.caret = remap(c.caret, at, removed, inserted);
                    }
                }
            }
        }
        self.group = None;
        let len = text.len_chars();
        normalize(&mut all, &mut prim, len, None);
        self.set_cursors(all, prim);
    }

    /// Puts a caret at `at`, which becomes the primary; or takes the one that is
    /// there away, if there is one, so the same click adds and removes.
    pub fn toggle_cursor_at(&mut self, at: usize, len: usize) {
        let at = at.min(len);
        let mut all = self.all_cursors();
        if all.len() > 1
            && let Some(i) = all.iter().position(|c| c.caret == at && c.is_empty())
        {
            all.remove(i);
            // The primary may have been the one removed: the last added takes over.
            let prim = if i == 0 { all.len() - 1 } else { 0 };
            let mut p = prim;
            normalize(&mut all, &mut p, len, None);
            self.set_cursors(all, p);
            return;
        }
        if all.len() >= MAX_CURSORS {
            return;
        }
        all.push(Cursor::new(at, at));
        let mut p = all.len() - 1;
        normalize(&mut all, &mut p, len, None);
        self.set_cursors(all, p);
    }

    /// Adds a caret one row above the topmost, or below the lowest, at the same
    /// horizontal place, which is what a column of carets is built from.
    pub(super) fn add_cursor_vertical(
        &mut self,
        ui: &egui::Ui,
        text: &Buffer,
        down: bool,
        wrap: bool,
    ) {
        let mut all = self.all_cursors();
        let len = text.len_chars();
        let mut p = 0;
        normalize(&mut all, &mut p, len, None);
        let edge = if down { all.len() - 1 } else { 0 };
        let src = all[edge];
        // The move is made by the one-caret code, on a stand-in for the edge caret.
        let saved = (self.anchor, self.caret, self.goal);
        self.anchor = src.caret;
        self.caret = src.caret;
        self.goal = src.goal;
        self.move_vertical(ui, text, if down { 1 } else { -1 }, false, wrap);
        let moved = Cursor {
            anchor: self.caret,
            caret: self.caret,
            goal: self.goal,
        };
        (self.anchor, self.caret, self.goal) = saved;
        if moved.caret == src.caret || all.len() >= MAX_CURSORS {
            // At the first or last line already: nothing to add.
            return;
        }
        // The edge caret remembers its column too, so the column holds as the stack grows.
        all[edge].goal = moved.goal;
        all.push(moved);
        let mut p = all.len() - 1;
        normalize(&mut all, &mut p, len, None);
        self.set_cursors(all, p);
        self.scroll_to_caret(text);
    }

    /// The word the caret is in, if there is one: a run of letters, digits and
    /// underscores. A double click selects a run of spaces too, but a caret in a gap
    /// has no word to look for.
    fn word_to_select(&self, text: &Buffer) -> Option<(usize, usize)> {
        let (a, b) = self.word_at(text, self.caret);
        let word = text.slice(a, b);
        (a < b && word.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some((a, b))
    }

    /// Ctrl+D: with nothing selected, selects the word the caret is in; with something
    /// selected, adds the next place the same text stands, wrapping round the end.
    pub fn add_next_occurrence(&mut self, text: &Buffer) {
        let n = text.len_chars();
        let (lo, hi) = self.selection(n);
        if lo == hi {
            if let Some((a, b)) = self.word_to_select(text) {
                self.anchor = a;
                self.caret = b;
            }
            return;
        }
        let needle = text.slice(lo, hi).into_owned();
        let width = hi - lo;
        let all = self.all_cursors();
        let taken = |start: usize| {
            all.iter()
                .any(|c| c.lo() == start && c.hi() == start + width)
        };
        let mut from = hi;
        let mut wrapped = false;
        for _ in 0..=all.len() + 1 {
            match text.find_from(&needle, from) {
                Some(start) if !taken(start) => {
                    self.add_selection(start, start + width, n);
                    self.scroll_to_caret(text);
                    return;
                }
                Some(start) => from = start + width,
                None if !wrapped => {
                    wrapped = true;
                    from = 0;
                }
                None => return,
            }
        }
    }

    /// Ctrl+Shift+L: a caret on every place the selected text, or the word the caret
    /// is in, stands.
    pub fn select_all_occurrences(&mut self, text: &Buffer) {
        let n = text.len_chars();
        let (mut lo, mut hi) = self.selection(n);
        if lo == hi {
            let Some((a, b)) = self.word_to_select(text) else {
                return;
            };
            (lo, hi) = (a, b);
        }
        let needle = text.slice(lo, hi).into_owned();
        let width = hi - lo;
        let mut found: Vec<Cursor> = Vec::new();
        let mut from = 0;
        while let Some(start) = text.find_from(&needle, from) {
            found.push(Cursor::new(start, start + width));
            from = start + width;
            if found.len() >= MAX_CURSORS {
                break;
            }
        }
        if found.is_empty() {
            return;
        }
        // The primary is the one that was selected, so the window stays with it.
        let mut p = found
            .iter()
            .position(|c| c.lo() == lo)
            .unwrap_or(found.len() - 1);
        normalize(&mut found, &mut p, n, None);
        self.set_cursors(found, p);
        self.scroll_to_caret(text);
    }

    /// The keys that are about several carets, and the running of the others for each
    /// of them. Returns whether the key was dealt with here.
    pub(super) fn multi_key(
        &mut self,
        ui: &egui::Ui,
        key: egui::Key,
        m: egui::Modifiers,
        text: &mut Buffer,
        opts: &Options,
        out: &mut Outcome,
    ) -> bool {
        use egui::Key as K;
        let ctrl = m.ctrl || m.command;
        let shift = m.shift;
        let editable = opts.editable;
        match key {
            // Escape puts the extra carets away, if there are any and the find bar is not
            // the thing that wanted the key.
            K::Escape if !self.extra.is_empty() && !self.find.open => {
                self.collapse_cursors();
                return true;
            }
            // Ctrl+D: the word, then the next place the same text stands.
            K::D if ctrl && !shift && !m.alt => {
                self.add_next_occurrence(text);
                return true;
            }
            K::L if ctrl && shift => {
                self.select_all_occurrences(text);
                return true;
            }
            K::ArrowUp | K::ArrowDown if ctrl && m.alt => {
                self.add_cursor_vertical(ui, text, key == K::ArrowDown, opts.wrap);
                return true;
            }
            // Select all is one selection.
            K::A if ctrl => {
                self.extra.clear();
                return false;
            }
            _ => {}
        }
        if self.extra.is_empty() {
            return false;
        }
        let per_caret = match key {
            K::ArrowLeft
            | K::ArrowRight
            | K::ArrowUp
            | K::ArrowDown
            | K::Home
            | K::End
            | K::PageUp
            | K::PageDown => true,
            K::Backspace | K::Delete | K::Enter | K::Tab => editable,
            K::Slash if ctrl => editable,
            K::K | K::D if ctrl && shift => editable,
            _ => false,
        };
        if !per_caret {
            return false;
        }
        // Whole-line commands act once per line, however many carets a line has.
        let raising = matches!(key, K::ArrowUp | K::ArrowDown) && (ctrl || m.alt);
        let any_selected = self.all_cursors().iter().any(|c| !c.is_empty());
        let by_line = raising
            || matches!(key, K::Slash | K::K | K::D) && ctrl
            || (key == K::Tab && any_selected);
        self.each(text, by_line, |e, t| e.key_single(ui, key, m, t, opts, out));
        self.scroll_to_caret(text);
        true
    }

    /// Adds a selection over `lo..hi` and makes it the primary.
    pub(super) fn add_selection(&mut self, lo: usize, hi: usize, len: usize) {
        let mut all = self.all_cursors();
        if all.len() >= MAX_CURSORS {
            return;
        }
        all.push(Cursor::new(lo, hi));
        let mut p = all.len() - 1;
        normalize(&mut all, &mut p, len, None);
        self.set_cursors(all, p);
    }

    /// What each selection holds, in the order they stand in the text.
    pub(super) fn selected_texts(&self, text: &Buffer) -> Vec<String> {
        let mut all = self.all_cursors();
        let mut p = 0;
        normalize(&mut all, &mut p, text.len_chars(), None);
        all.iter()
            .filter(|c| !c.is_empty())
            .map(|c| text.slice(c.lo(), c.hi()).into_owned())
            .collect()
    }

    /// What the clipboard should hold for a copy: the selections one to a line, or if
    /// none has anything selected, the lines the carets are on.
    pub(super) fn copy_text(&self, text: &Buffer) -> String {
        let picked = self.selected_texts(text);
        if !picked.is_empty() {
            return picked.join("\n");
        }
        let mut all = self.all_cursors();
        let mut p = 0;
        normalize(&mut all, &mut p, text.len_chars(), Some(text));
        let n = text.len_chars();
        let mut s = String::new();
        for c in &all {
            let (_, _, range) = Editor::line_range(text, c.lo(), c.hi());
            let (a, b) = (range.start.min(n), range.end.min(n));
            if a < b {
                s.push_str(&text.slice(a, b));
            }
        }
        s
    }

    /// Pastes `s`. If it has as many lines as there are carets, each gets one, in order;
    /// otherwise every caret gets all of it.
    pub(super) fn paste_each(&mut self, text: &mut Buffer, s: &str, out: &mut Outcome) {
        let count = {
            let mut all = self.all_cursors();
            let mut p = 0;
            normalize(&mut all, &mut p, text.len_chars(), None);
            all.len()
        };
        // Line breaks of every kind are one kind, before the lines are counted.
        let unified = s.replace("\r\n", "\n").replace('\r', "\n");
        let s = unified.as_str();
        let body = s.strip_suffix('\n').unwrap_or(s);
        let lines: Vec<&str> = body.split('\n').collect();
        if count > 1 && lines.len() == count && !body.is_empty() {
            // The carets are run last first, so the first line goes to the last call.
            let mut taken = 0usize;
            self.each(text, false, |e, t| {
                let line = lines[count - 1 - taken];
                taken += 1;
                e.insert(t, line, out);
            });
        } else {
            self.each(text, false, |e, t| e.insert(t, s, out));
        }
    }
}

/// A list of cursors frozen as a [`Snap`], with the primary marked.
struct Snapshot(Snap);

impl Snapshot {
    fn of(all: &[Cursor], primary: usize) -> Snapshot {
        Snapshot(Snap {
            primary: (all[primary].anchor, all[primary].caret),
            extra: all
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != primary)
                .map(|(_, c)| (c.anchor, c.caret))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(a: usize, b: usize) -> Cursor {
        Cursor::new(a, b)
    }

    fn norm(v: &[(usize, usize)], primary: usize, len: usize) -> (Vec<(usize, usize)>, usize) {
        let mut all: Vec<Cursor> = v.iter().map(|&(a, b)| c(a, b)).collect();
        let mut p = primary;
        normalize(&mut all, &mut p, len, None);
        (all.iter().map(|c| (c.anchor, c.caret)).collect(), p)
    }

    #[test]
    fn a_position_before_a_change_stays() {
        assert_eq!(remap(3, 10, 2, 5), 3);
        assert_eq!(remap(9, 10, 0, 4), 9);
    }

    #[test]
    fn a_position_after_a_change_moves_by_what_it_added_or_took() {
        assert_eq!(remap(20, 10, 2, 5), 23);
        assert_eq!(remap(20, 10, 5, 0), 15);
        assert_eq!(remap(12, 10, 2, 5), 15, "just past what was removed");
    }

    #[test]
    fn a_position_inside_what_was_removed_goes_to_where_the_replacement_ends() {
        assert_eq!(remap(11, 10, 4, 0), 10);
        assert_eq!(remap(11, 10, 4, 3), 13);
    }

    #[test]
    fn a_position_at_the_start_of_an_insertion_stays_in_front_of_it() {
        assert_eq!(remap(10, 10, 0, 3), 10);
    }

    #[test]
    fn a_position_at_the_start_of_a_replacement_goes_to_its_end() {
        assert_eq!(remap(10, 10, 2, 3), 13);
    }

    #[test]
    fn cursors_are_put_in_order() {
        assert_eq!(
            norm(&[(9, 9), (2, 2), (5, 5)], 0, 20).0,
            vec![(2, 2), (5, 5), (9, 9)]
        );
    }

    #[test]
    fn the_primary_is_followed_through_the_sort() {
        let (_, p) = norm(&[(9, 9), (2, 2), (5, 5)], 0, 20);
        assert_eq!(p, 2, "the one that was at 9 is now last");
        let (_, p) = norm(&[(9, 9), (2, 2), (5, 5)], 1, 20);
        assert_eq!(p, 0);
    }

    #[test]
    fn two_carets_in_one_place_become_one() {
        let (v, p) = norm(&[(4, 4), (4, 4), (8, 8)], 1, 20);
        assert_eq!(v, vec![(4, 4), (8, 8)]);
        assert_eq!(p, 0, "and the primary is the one they became");
    }

    #[test]
    fn overlapping_selections_are_joined() {
        assert_eq!(norm(&[(2, 6), (4, 9)], 0, 20).0, vec![(2, 9)]);
        assert_eq!(norm(&[(2, 9), (4, 6)], 0, 20).0, vec![(2, 9)]);
    }

    #[test]
    fn a_selection_going_backwards_stays_backwards_when_joined() {
        assert_eq!(norm(&[(6, 2), (5, 8)], 0, 20).0, vec![(8, 2)]);
    }

    #[test]
    fn selections_that_only_touch_stay_apart_but_a_caret_on_one_edge_joins_it() {
        assert_eq!(norm(&[(2, 5), (5, 8)], 0, 20).0, vec![(2, 5), (5, 8)]);
        assert_eq!(norm(&[(2, 5), (5, 5)], 0, 20).0, vec![(2, 5)]);
        assert_eq!(norm(&[(5, 5), (5, 8)], 0, 20).0, vec![(5, 8)]);
    }

    #[test]
    fn cursors_are_held_inside_the_document() {
        assert_eq!(norm(&[(50, 60), (3, 3)], 0, 10).0, vec![(3, 3), (10, 10)]);
    }

    #[test]
    fn with_lines_given_cursors_on_one_line_are_joined() {
        let text = Buffer::from("abc def\nghi jkl\nmno");
        let mut all = vec![c(1, 1), c(5, 5), c(9, 9), c(17, 17)];
        let mut p = 0;
        normalize(&mut all, &mut p, text.len_chars(), Some(&text));
        assert_eq!(all.len(), 3, "the two on the first line are one");
    }

    #[test]
    fn a_snapshot_marks_the_primary() {
        let s = Snapshot::of(&[c(1, 1), c(5, 7), c(9, 9)], 1).0;
        assert_eq!(s.primary, (5, 7));
        assert_eq!(s.extra, vec![(1, 1), (9, 9)]);
    }
}
