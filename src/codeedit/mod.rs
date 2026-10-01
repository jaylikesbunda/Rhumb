//! A code editor that only shapes what is on screen.
//!
//! `egui_code_editor` re-lays-out and re-tokenizes the whole document every
//! frame, so its cost grows with the file rather than the screen: 6,000 lines
//! measured about 18 ms a frame while completely idle, which is more than a
//! whole 60 fps frame's budget before a key is pressed. No setting changes
//! that, and the crate has no faster alternative.
//!
//! So this is written from scratch, and the whole design follows from one
//! rule: **nothing here is O(document) per frame.** The text is a rope, which
//! answers "which line is this" and "where does that line start" in O(log n),
//! and only the lines inside the viewport are shaped.
//!
//! The subtle part, and the part that is easy to get wrong, is that a layout
//! job built from visible lines only has *window-local* character indices.
//! `Galley::cursor_from_pos` and `pos_from_cursor` speak in those, not in
//! document characters, so every hit test and every caret rectangle has to
//! translate through [`Window`]. With soft wrap on, a logical line occupies
//! several visual rows, so the offset is per row rather than per line.
//!
//! The modules split so each can be tested without the others:
//!
//! - [`crate::buffer`] — the rope the text lives in. Pure, no egui.
//! - [`markup`] — colouring one line. Pure, no egui.
//! - the rest of this file — the widget itself.

/// A position in points moved to the nearest whole physical pixel.
///
/// Text is snapped to pixels when it is painted, each piece on its own, so two
/// pieces at a fractional offset can round in different directions and drift a
/// pixel against each other while the window scrolls. Snapping the origin once
/// makes them all move together.
fn snap_to_pixel(ui: &egui::Ui, v: f32) -> f32 {
    let ppp = ui.ctx().pixels_per_point();
    (v * ppp).round() / ppp
}

/// The specification: what the editor does, driven through real input.
#[cfg(test)]
mod behaviour;
mod find;
/// Drives the editor through real input events, for the tests above.
#[cfg(test)]
mod harness;
mod highlight;
mod markup;
mod multi;
#[cfg(test)]
mod multi_tests;
#[cfg(test)]
mod sweep;
mod window;

use crate::buffer::{Buffer, Edit};
#[cfg(test)]
pub use markup::tokenize;
pub use markup::{Lang, State, Token, tokenize_with};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
pub use window::Window;

// ---- what the caller asks for ---------------------------------------------

/// What the editor shows, decided by the caller each frame.
#[derive(Clone, Debug)]
pub struct Options {
    /// Line numbers down the left edge.
    pub line_numbers: bool,
    /// Soft-wrap long lines instead of letting them run past the edge.
    pub wrap: bool,
    /// Tint comments, strings, numbers and keywords.
    pub highlight: bool,
    /// Allow edits. A read-only document still scrolls and selects.
    pub editable: bool,
    /// Line-comment marker for the language, e.g. `//` or `#`.
    pub comment: &'static str,
    /// What the language can carry across a line break: block comments, triple
    /// quotes. Decides how the text is coloured; `comment` decides what Ctrl+/ types.
    pub lang: Lang,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            line_numbers: true,
            wrap: false,
            highlight: true,
            editable: true,
            comment: "//",
            lang: Lang::line_only("//"),
        }
    }
}

/// What happened, so the caller can do its own editing work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The buffer changed this frame.
    pub edited: bool,
    /// A key the app owns, which the editor deliberately ignored.
    pub handled_elsewhere: bool,
    /// The editor took this frame's clipboard shortcut.
    ///
    /// The app also wants Ctrl+C, Ctrl+X and Ctrl+V for the file list, and it
    /// cannot tell whether they were pressed by looking for the key, because
    /// the window layer turns them into events instead. So the editor says so.
    pub clipboard: bool,
}

// ---- find in the document ---------------------------------------------------

/// The find bar, and what it has found.
///
/// Owned by the editor because the hits are in the editor's buffer and in the
/// editor's units; a find that lived in the app would have to be handed the whole
/// file on every keystroke to be worth anything.
#[derive(Clone, Debug, Default)]
pub struct Finder {
    /// Whether the bar is on screen.
    pub open: bool,
    /// What has been typed.
    pub needle: String,
    /// Whether letters must match exactly.
    pub case: bool,
    /// Whether a hit must be a whole word.
    pub whole: bool,
    /// The hits, as character ranges, in order.
    pub hits: Vec<find::Hit>,
    /// Which of them is the current one.
    pub current: usize,
    /// Whether there were more hits than are held.
    pub capped: bool,
    /// Set by the bar when it has moved the selection and the window has not yet
    /// caught up. See `find_sync`, which is what acts on it.
    pub needs_scroll: bool,
    /// Set when the text has changed under the hits, so they are re-found.
    ///
    /// A flag rather than a comparison of the whole buffer against a copy of it:
    /// keeping a second copy of a multi-megabyte file alive to notice an edit is
    /// the exact cost the editor exists to avoid.
    pub stale: bool,
    /// What a replace puts in.
    pub replacement: String,
    /// Whether the replace row is showing.
    pub replace: bool,
    /// Whether the document can be edited at all, set each frame by the editor.
    pub can_replace: bool,
    /// A replace the bar asked for, applied by the editor where the buffer can be
    /// written to: `Some(false)` is the current match, `Some(true)` is all of them.
    pub pending: Option<bool>,
    /// The answer of a search running on another thread, once it has one.
    job: Option<Arc<Mutex<Option<Found>>>>,
    /// Bumped by every new search, so an older one knows to stop.
    generation: Arc<AtomicU64>,
    /// Whether a search on another thread has not answered yet.
    pub searching: bool,
    /// Wakes the window when a background search answers. Handed over by the
    /// editor each frame, because a `Finder` is made before there is a window.
    ctx: Option<egui::Context>,
    /// Whether the answer should be selected when it arrives, as it is when a
    /// needle is typed. Not for a re-search after an edit, which must leave the
    /// caret where the reader put it.
    jump: bool,
    /// Where the caret was left by a replace, so the next match is the first one
    /// after it once the answer arrives.
    after: Option<usize>,
    /// What the hits held now were found for: the needle, and the two options. Kept
    /// so a needle that has only grown can be searched for on the lines that already
    /// matched. `None` while a search is in flight or the hits are out of date.
    searched: Option<(String, bool, bool)>,
    /// The same, for the search on another thread, until it answers.
    job_query: Option<(String, bool, bool)>,
}

/// What a background search found: the hits, and whether there were more.
type Found = (Vec<find::Hit>, bool);

/// Text above this many bytes is searched on another thread. Below it a search
/// finishes well inside a frame — about twenty milliseconds a megabyte — and answering
/// at once keeps stepping and counting instant.
const SEARCH_ASYNC_BYTES: usize = 256 * 1024;

impl Finder {
    /// The current hit, if there is one.
    pub fn selected(&self) -> Option<find::Hit> {
        self.hits.get(self.current).copied()
    }

    /// Re-runs the search, from scratch, against the whole document.
    ///
    /// Called when the needle or the options change and when the text has moved
    /// under the hits. A full re-scan rather than an incremental adjustment,
    /// because "the text changed" is exactly the case where an adjustment cannot
    /// be trusted: the character index of every hit after the edit is different.
    ///
    /// A large document is scanned on another thread over a copy of the rope, and
    /// the hits arrive a few frames later through [`Finder::poll`].
    pub fn refresh(&mut self, text: &Buffer) {
        let query = find::Query {
            case: self.case,
            whole: self.whole,
        };
        // Whatever was running is about to be answering a different question.
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.job = None;
        self.searching = false;
        // Asked for by the caller, and only good for this one search.
        let jump = std::mem::take(&mut self.jump);
        let after = self.after.take();
        // A needle that has only grown, over text that has not changed, can only match
        // where the shorter one did. So the lines that had a match are searched, and
        // the rest of the document is not read at all: typing a word into the find box
        // costs a full scan for its first letter and nearly nothing for the rest.
        let this = (self.needle.clone(), self.case, self.whole);
        let narrowed = !self.stale
            && !self.capped
            && !self.needle.is_empty()
            && self.searched.as_ref().is_some_and(|(old, case, whole)| {
                *case == this.1
                    && *whole == this.2
                    && !old.is_empty()
                    && self.needle.starts_with(old.as_str())
                    && *old != self.needle
            });
        if narrowed {
            let mut lines: Vec<usize> = self.hits.iter().map(|h| text.line_of_char(h.0)).collect();
            lines.dedup();
            self.hits = find::find_in_lines(text, &lines, &self.needle, query);
            self.capped = find::is_capped(&self.hits);
            self.current = self.current.min(self.hits.len().saturating_sub(1));
            self.searched = Some(this);
            return;
        }
        self.searched = None;
        if text.len_bytes() > SEARCH_ASYNC_BYTES
            && !self.needle.is_empty()
            && let Some(ctx) = self.ctx.clone()
        {
            let shared: Arc<Mutex<Option<Found>>> = Arc::new(Mutex::new(None));
            self.job = Some(Arc::clone(&shared));
            self.job_query = Some(this);
            self.searching = true;
            self.jump = jump;
            self.after = after;
            self.hits.clear();
            self.capped = false;
            self.stale = false;
            let snapshot = text.snapshot();
            let needle = self.needle.clone();
            let generation = Arc::clone(&self.generation);
            let wanted = generation.load(Ordering::Relaxed);
            std::thread::spawn(move || {
                let cancelled = || generation.load(Ordering::Relaxed) != wanted;
                let hits = find::find_all_buffer_until(&snapshot, &needle, query, &cancelled);
                if cancelled() {
                    return;
                }
                let capped = find::is_capped(&hits);
                if let Ok(mut slot) = shared.lock() {
                    *slot = Some((hits, capped));
                }
                ctx.request_repaint();
            });
            return;
        }
        self.hits = find::find_all_buffer(text, &self.needle, query);
        self.capped = find::is_capped(&self.hits);
        self.stale = false;
        self.searched = Some(this);
        // Clamped rather than reset: a reader who has stepped to the third of
        // forty hits and then types one more letter has not asked to go back to
        // the first. The clamp is what stops a shorter result leaving the
        // selection pointing at a hit that is no longer there.
        self.current = self.current.min(self.hits.len().saturating_sub(1));
    }

    /// Takes in the answer of a search running on another thread, if it has come.
    ///
    /// `Some(jump)` once the hits have arrived, where `jump` says whether the
    /// caller should select the current one, as it would have straight after a
    /// search that finished in the frame.
    pub fn poll(&mut self) -> Option<bool> {
        let job = self.job.as_ref()?;
        let found = job.lock().ok()?.take()?;
        self.job = None;
        self.searching = false;
        (self.hits, self.capped) = found;
        self.stale = false;
        self.searched = self.job_query.take();
        match self.after.take() {
            Some(from) => {
                self.current = self
                    .hits
                    .iter()
                    .position(|&(lo, _)| lo >= from)
                    .unwrap_or(0);
            }
            None => self.current = self.current.min(self.hits.len().saturating_sub(1)),
        }
        Some(std::mem::take(&mut self.jump))
    }

    /// Opens the bar, optionally with something already in it.
    ///
    /// A needle is only put in when there is no text selected, so Ctrl+F with a
    /// selection searches for what is selected — which is what every other editor
    /// does and is the fastest way to ask "where else is this?".
    pub fn open_with(&mut self, selected: Option<String>) {
        if let Some(text) = selected.filter(|s| !s.is_empty() && !s.contains('\n')) {
            self.needle = text;
        }
        self.open = true;
    }

    /// Moves to the next or previous hit, wrapping round, and returns it.
    pub fn step(&mut self, forward: bool) -> Option<find::Hit> {
        let from = if forward {
            self.selected().map_or(0, |(_, hi)| hi)
        } else {
            self.selected().map_or(0, |(lo, _)| lo)
        };
        let (index, hit) = find::next(&self.hits, from, forward)?;
        self.current = index;
        Some(hit)
    }
}

// ---- the editor's own state ------------------------------------------------

/// The part of a long line that is shaped.
struct LongSlice {
    /// The first and one past the last column shaped.
    c0: usize,
    c1: usize,
    /// How far in from the line's left edge the first shaped column sits, in points.
    lead: f32,
    /// The whole line's width, in points.
    full: f32,
}

/// One undoable step: the edits it made, in order, and where the caret and
/// selection were before and after.
///
/// A step costs the size of what was typed, not the size of the file. Undo
/// replays the edits backwards and redo forwards.
#[derive(Clone)]
struct Step {
    edits: Vec<Edit>,
    /// Every selection before the step, and after it.
    before: multi::Snap,
    after: multi::Snap,
}

/// Where the carets were when the current step began.
struct Open {
    snap: multi::Snap,
}

/// A line longer than this many characters, without wrapping, is shaped only where it
/// is on screen. Shaping and drawing a line costs in proportion to its length, so a
/// hundred kilobytes on one line — a minified script, a data file — would otherwise
/// cost tens of milliseconds a frame and a keystroke, to draw a few hundred characters.
const LONG_LINE: usize = 4_000;

/// The slice of a long line that is shaped starts and ends on multiples of this many
/// columns, so scrolling sideways reshapes it once every so many columns and not on
/// every pixel.
const SLICE_STEP: usize = 256;

/// Room left past the end of the longest line when scrolled all the way right, in points.
const END_PAD: f32 = 12.0;

/// How far past each side of the pane, in points, a long line's slice reaches.
const SLICE_MARGIN: f32 = 700.0;

/// How long, in seconds, the window takes to cover most of the way to where the
/// wheel sent it. Longer is smoother and lags more; this is about a tenth of a
/// second to be two thirds of the way there.
const WHEEL_EASE: f32 = 0.09;

/// A burst of typing within this window counts as one undo step.
const UNDO_COALESCE: std::time::Duration = std::time::Duration::from_millis(600);
/// Undo steps kept before the oldest is dropped.
const UNDO_LIMIT: usize = 200;

/// The editor's state.
///
/// Nothing in here is O(document): the text is a rope, and every edit, line
/// lookup and character count is O(log n).
#[derive(Default)]
pub struct Editor {
    caret: usize,
    anchor: usize,
    /// The selections beside the primary one, if there is more than one caret.
    extra: Vec<multi::Cursor>,
    /// Set while a command is being run for every caret in turn.
    group: Option<multi::Group>,
    /// First logical line shown.
    top_line: usize,
    /// How many rows fit on screen, worked out each frame from the rect.
    ///
    /// Kept here because page-up, page-down and following the caret with the
    /// arrow keys all need it, and it is view state like the scroll position.
    rows_visible: usize,
    /// Caret blink phase, in seconds.
    blink: f32,
    focused: bool,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The step being built up, if there is one.
    open: Option<Open>,
    /// When the last edit landed, which is what decides whether the next one
    /// joins it or starts a step of its own.
    last_edit: Option<std::time::Instant>,
    /// Wheel movement not yet worth a whole line, in points.
    /// How far the window is scrolled, in points.
    ///
    /// The window's position, in the same unit the wheel reports and the rows are
    /// drawn in. The line it starts at is *derived* from this, not the other way
    /// round: a window that could only be positioned by whole line could not be
    /// scrolled smoothly, because every wheel movement would have to be rounded to
    /// a line, and rounding is exactly what makes a scroll feel like a staircase.
    scroll_y: f32,
    /// How far the wheel has asked the window to go that it has not gone yet, in
    /// points, down positive. Each frame covers a share of it, so a run of notches
    /// blends into one steady motion instead of a step per notch.
    scroll_pending: f32,
    /// How many points into the first shown line the window starts. The window is
    /// really anchored at a line and this offset into it, not at a single distance
    /// from the top: with wrapping on, lines are different heights, and a distance
    /// cannot say where the top of the window is without measuring every line above
    /// it. `scroll_y` is kept as an estimate of that position for the scrollbar.
    top_off: f32,
    /// The average height of a line as last drawn, for the scrollbar and for turning
    /// a distance into a line. One row per line unless wrapping makes them taller.
    line_h_est: f32,
    /// Whether the text wraps, as of this frame.
    wrapping: bool,
    /// The caret has moved and the window has not yet been brought to it. Only used
    /// while wrapping: bringing a caret into view there means measuring lines, which
    /// needs the `Ui` that only the frame has.
    reveal_caret: bool,
    /// The height of one row, remembered from the last frame so a scroll position
    /// can be turned back into a line without asking the font.
    scroll_h: f32,
    /// The height of the pane, likewise, for the same reason.
    view_h: f32,
    /// How far the text is pushed sideways, in points. Always zero while the text
    /// wraps, because a wrapped line is never wider than the pane.
    scroll_x: f32,
    /// The scrollbar's thumb was grabbed and the button has not been released.
    scrollbar_grabbed: bool,
    /// How far into the thumb the pointer was when it was grabbed, in points.
    scrollbar_grab: f32,
    /// The rows drawn last frame, kept so the tests can check the mapping the
    /// drawing was built from. Not read by anything else.
    last_rows: Vec<RowSpan>,
    /// Find in the document.
    pub find: Finder,
    /// What state each line starts in, for colouring.
    hl: highlight::Highlights,
    /// Where the caret was horizontally when a run of Up and Down began, and the
    /// caret position that run left it at. Only good while the caret is still there.
    goal: Option<(usize, f32)>,
    /// The width text wraps at, as of the last frame, for laying out a single line.
    wrap_w: f32,
    /// Advance widths of the characters that are not plain ASCII, looked up from the
    /// font once and remembered, for measuring long lines without shaping them.
    wide_w: std::collections::HashMap<char, f32>,
    /// The full width of the last long line measured: its line, its length, the edit
    /// generation, and the width. A long line is measured whole for the scroll range,
    /// and that is a pass over every character, so it is kept until something changes.
    long_width: Option<(usize, usize, u64, f32)>,
    /// Bumped whenever the text is edited, to tell a remembered measurement that the
    /// text under it has changed.
    edit_gen: u64,
    /// How far the scrollbar has grown, from 0 (thin, at rest) to 1 (full width, with
    /// the pointer close). Animated, so it eases out to meet the pointer.
    bar_grow: f32,
    /// The caret position the window was last scrolled sideways to show. Scrolling to
    /// the caret happens when the caret moves, not on every frame: a frame that
    /// dragged the view back to the caret would make scrolling sideways impossible.
    followed_caret: Option<usize>,
    /// How many characters the document had when it was last looked at.
    len: usize,
    /// How many lines it had, likewise. Kept so the scroll arithmetic, which has no
    /// buffer in hand, can ask without being handed one.
    lines: usize,
    /// Height of the find bar last frame, so the text knows to start below it.
    find_h: f32,
    /// Where the find bar's buttons went last frame, in the order they are laid
    /// out. Recorded rather than recomputed so a test can find one by name.
    find_buttons: Vec<egui::Rect>,
    /// The text the editor laid out last frame, for the tests to identify its own
    /// galley among everything else on the frame. Not read in a release build.
    #[cfg(test)]
    shaped: String,
    /// Where that text starts, likewise for the tests: an empty document lays out
    /// no galley at all, so a test cannot measure the text's left edge from the
    /// drawing and has to be told.
    #[cfg(test)]
    origin: egui::Pos2,
}

impl Editor {
    /// Where the top of the window is, in lines: the whole number is the line it starts
    /// at and the fraction how far into it.
    pub fn scroll_line(&self) -> f32 {
        self.scroll_y / self.est_h()
    }

    /// Scrolls to a place given in lines, straight there.
    pub fn set_scroll_line(&mut self, line: f32) {
        self.scroll_pending = 0.0;
        self.set_scroll(line.max(0.0) * self.est_h());
    }

    /// How far the window is scrolled, in points. For tests.
    #[cfg(test)]
    pub fn scroll_y_for_test(&self) -> f32 {
        self.scroll_y
    }

    /// How far down the document the window is, from 0 at the top to 1 at the furthest
    /// it can be scrolled.
    pub fn scroll_fraction(&self) -> f32 {
        let max = self.max_scroll_y();
        if max > 0.0 {
            (self.scroll_y / max).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Scrolls to a fraction of the way down, straight there and without easing, for
    /// something else that is being scrolled to pull the editor along.
    pub fn set_scroll_fraction(&mut self, fraction: f32) {
        self.scroll_pending = 0.0;
        self.set_scroll(fraction.clamp(0.0, 1.0) * self.max_scroll_y());
    }

    /// Where the caret is, as a character index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn caret_index(&self) -> usize {
        self.caret
    }

    /// How far the window is scrolled down, in points.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn scroll_offset(&self) -> f32 {
        self.scroll_y
    }

    /// The rows drawn on the last frame: the character range each one covers and
    /// the line it belongs to.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn last_rows(&self) -> &[RowSpan] {
        &self.last_rows
    }
}

impl Editor {
    /// Whether the editor has the keyboard, as egui's focus system sees it.
    ///
    /// The app needs this to decide who owns the clipboard shortcuts and the
    /// selection keys, which both mean different things depending on whether
    /// the caret or the file list is active.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// The selected range, normalised, in document characters.
    ///
    /// Clamped to `len` characters, because a document that ends in a newline
    /// has one more character *index* than it has characters, and a selection
    /// reaching that index is really reaching the end of the text. Every caller
    /// then knows its range addresses real characters without having to think
    /// about the difference.
    pub fn selection(&self, len: usize) -> (usize, usize) {
        let (lo, hi) = if self.anchor <= self.caret {
            (self.anchor, self.caret)
        } else {
            (self.caret, self.anchor)
        };
        (lo.min(len), hi.min(len))
    }

    /// Whether anything is selected.
    pub fn has_selection(&self, len: usize) -> bool {
        self.selection(len).0 != self.selection(len).1
    }

    /// Moves the caret and drops the selection.
    ///
    /// The position is clamped to `len` characters rather than trusted, because
    /// the buffer is edited in place and a position set before an edit can be one
    /// character out of date by the time it is read.
    pub fn set_caret(&mut self, caret: usize, len: usize) {
        self.caret = caret.min(len);
        self.anchor = self.caret;
        self.extra.clear();
    }

    /// Selects a range, caret at the end, both clamped to `len` characters.
    pub fn select(&mut self, lo: usize, hi: usize, len: usize) {
        self.anchor = lo.min(len);
        self.caret = hi.min(len);
        self.extra.clear();
    }

    /// Notes how long the document is, for the readers that only have the editor.
    ///
    /// O(1): the rope keeps its own count, and there is no line index to rebuild —
    /// the rope answers "which line is this" and "where does that line start"
    /// itself, exactly, after every edit.
    fn ensure_index(&mut self, text: &Buffer) {
        self.len = text.len_chars();
        self.lines = text.lines();
    }

    /// How many characters the document has, as of the last frame.
    ///
    /// One frame stale after an edit, which is why nothing that has to be exact
    /// uses it: those places count for themselves, where they already have the
    /// text in hand.
    pub fn len(&self) -> usize {
        self.len
    }

    // ---- undo -------------------------------------------------------------

    fn push_undo(&mut self, text: &mut Buffer) {
        // One step for a command run over several carets, opened by the first of them
        // to edit and left alone by the rest, with the state from before any of them.
        if let Some(g) = &mut self.group {
            if g.started {
                return;
            }
            g.started = true;
        }
        let now = std::time::Instant::now();
        let burst = self
            .last_edit
            .is_some_and(|t| now.duration_since(t) <= UNDO_COALESCE);
        if !burst {
            // The step that was open is finished.
            self.seal(text);
            self.open = Some(Open {
                snap: self.snapshot(),
            });
            self.redo.clear();
        }
        self.last_edit = Some(now);
    }

    /// Closes the step that is open, taking the edits the buffer has journaled
    /// since it began. A step that changed nothing is dropped.
    fn seal(&mut self, text: &mut Buffer) {
        // A finished step ends the burst, so an edit that follows starts a step of
        // its own instead of being folded into one that has already been closed.
        self.last_edit = None;
        let edits = text.take_journal();
        match self.open.take() {
            Some(open) if !edits.is_empty() => {
                self.undo.push(Step {
                    edits,
                    before: open.snap,
                    after: self.snapshot(),
                });
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                }
            }
            Some(_) => {}
            // Edits nobody opened a step for. The history no longer describes the
            // buffer, and replaying it would edit text that is not there.
            None if !edits.is_empty() => {
                self.undo.clear();
                self.redo.clear();
            }
            None => {}
        }
    }

    /// Steps the buffer through its undo history.
    pub fn undo_redo(&mut self, text: &mut Buffer, undo: bool) -> bool {
        // Whatever step is still open is finished first, so it can be undone.
        self.seal(text);
        let step = if undo {
            self.undo.pop()
        } else {
            self.redo.pop()
        };
        let Some(step) = step else { return false };
        // Undo replays the edits backwards, redo forwards. Each is checked against
        // the buffer first: a step is only ever applied to the text it was made
        // from. If one does not fit, whatever was already applied is put back and
        // the history is dropped, rather than editing text that is not there.
        let applied = if undo {
            let mut done = 0;
            for e in step.edits.iter().rev() {
                if !text.holds_inserted(e) {
                    break;
                }
                text.undo_edit(e);
                done += 1;
            }
            (done == step.edits.len()).then_some(()).ok_or(done)
        } else {
            let mut done = 0;
            for e in &step.edits {
                if !text.holds_removed(e) {
                    break;
                }
                text.redo_edit(e);
                done += 1;
            }
            (done == step.edits.len()).then_some(()).ok_or(done)
        };
        if let Err(done) = applied {
            if undo {
                for e in step.edits.iter().rev().take(done).rev() {
                    text.redo_edit(e);
                }
            } else {
                for e in step.edits.iter().take(done).rev() {
                    text.undo_edit(e);
                }
            }
            self.undo.clear();
            self.redo.clear();
            return false;
        }
        let n = text.len_chars();
        let snap = if undo { &step.before } else { &step.after };
        self.restore(snap, n);
        if undo {
            self.redo.push(step);
        } else {
            self.undo.push(step);
        }
        self.last_edit = None;
        true
    }

    // ---- editing ----------------------------------------------------------

    /// Replaces the selection, or inserts at the caret.
    pub fn insert(&mut self, text: &mut Buffer, s: &str, out: &mut Outcome) {
        self.push_undo(text);
        self.insert_undoed(text, s, out);
    }

    /// Backspace, or forward delete when there is no selection.
    pub fn delete(&mut self, text: &mut Buffer, back: bool, out: &mut Outcome) {
        self.delete_range(text, back, out, false)
    }

    /// Backspace or Delete, or a whole word of them with the word modifier.
    ///
    /// The word is the same one the word-wise caret movement crosses, so
    /// Ctrl+Backspace removes exactly what Ctrl+Left would have walked over. It
    /// is worked out by asking [`Editor::word_step`] where the caret would land
    /// and removing everything between, rather than by a second definition of
    /// "a word" that could drift from the first.
    pub fn delete_word(&mut self, text: &mut Buffer, back: bool, out: &mut Outcome) {
        self.delete_range(text, back, out, true)
    }

    fn delete_range(&mut self, text: &mut Buffer, back: bool, out: &mut Outcome, word: bool) {
        let n = text.len_chars();
        let (lo, hi) = self.selection(n);
        if lo != hi {
            self.push_undo(text);
            text.remove(lo, hi);
            self.set_caret(lo, text.len_chars());
            out.edited = true;
            return;
        }
        if back {
            if lo == 0 {
                return;
            }
            self.push_undo(text);
            let to = if word {
                self.word_step(text, lo, -1)
            } else {
                crate::editing::cluster_start(text, lo)
            };
            // Backspace between a bracket and the closer it brought with it takes both.
            let end = if !word && crate::editing::inside_empty_pair(text, lo) {
                lo + 1
            } else {
                lo
            };
            text.remove(to, end);
            self.set_caret(to, text.len_chars());
        } else {
            if lo >= n {
                return;
            }
            self.push_undo(text);
            let to = if word {
                self.word_step(text, lo, 1)
            } else {
                crate::editing::cluster_end(text, lo)
            };
            text.remove(lo, to);
            self.set_caret(lo, text.len_chars());
        }
        out.edited = true;
    }

    /// The range of whole lines the selection touches, and the caret position
    /// to leave behind.
    ///
    /// Shared by the line commands — delete, duplicate, comment — because they
    /// all have to agree on which lines "the selection" means, and three
    /// hand-written answers to that question is how they come to disagree.
    ///
    /// The range *includes* the newline after the last line, so deleting it
    /// takes the line with it rather than leaving a blank one behind. The last
    /// line of a document has no newline, and that case is handled by the caller
    /// clamping to the document's end.
    fn line_range(text: &Buffer, lo: usize, hi: usize) -> (usize, usize, std::ops::Range<usize>) {
        let first = text.line_of_char(lo);
        let last = text.line_of_char(hi);
        let start = text.line_start(first);
        // One past the end of the last line's content, which for every line but
        // the last is the newline itself.
        let end = text.line_end(last) + 1;
        (first, last, start..end.min(text.len_chars() + 1))
    }

    /// Deletes the line or lines the caret is on, newline and all.
    pub fn delete_lines(&mut self, text: &mut Buffer, out: &mut Outcome) {
        let n = text.len_chars();
        let (lo, hi) = self.selection(n);
        let (_, _, range) = Self::line_range(text, lo, hi);
        let (a, b) = (range.start.min(n), range.end.min(n));
        // Nothing to take: an empty document, or a line that is already at the
        // end. Pushing an undo step for an edit that changes nothing would leave
        // one Ctrl+Z doing nothing visible, and the next one undoing something
        // else — so the document looks like it has lost a keystroke.
        if a >= b {
            return;
        }
        self.push_undo(text);
        text.remove(a, b);
        self.set_caret(range.start, text.len_chars());
        out.edited = true;
    }

    /// Duplicates the line or lines the caret is on, below themselves.
    ///
    /// The copy goes underneath rather than above, because the caret stays where
    /// it was and a line duplicated upwards moves the text the reader is looking
    /// at. The copy is part of the same undo step, so one Ctrl+Z takes both away.
    pub fn duplicate_lines(&mut self, text: &mut Buffer, out: &mut Outcome) {
        let n = text.len_chars();
        let (lo, hi) = self.selection(n);
        let (_, _, range) = Self::line_range(text, lo, hi);
        let copy = text.slice(range.start, range.end.min(n)).into_owned();
        if copy.is_empty() {
            return;
        }
        self.push_undo(text);
        // The range ends past the newline that ends the last line, so the copy
        // re-inserts that newline and the two lines come out in order. A document
        // whose last line has no newline gets one here, which is the same thing
        // pressing Enter at the end of a file does.
        let at = range.end.min(n);
        let copy = if range.end > n {
            format!("\n{copy}")
        } else {
            copy
        };
        text.insert(at, &copy);
        self.set_caret(range.start, text.len_chars());
        out.edited = true;
    }

    /// Tab, or Shift+Tab over a selection. Falls through to the shared
    /// indenter so tab and the command key cannot disagree.
    pub fn tab(&mut self, text: &mut Buffer, outdent: bool, out: &mut Outcome) {
        if !self.has_selection(text.len_chars()) {
            if !outdent {
                self.insert(text, "    ", out);
                return;
            }
            // Shift+Tab with nothing selected takes one level of indent off the line
            // the caret is on, as it does everywhere else. The caret moves left by
            // however much came off, but never past the start of the line.
            let before = text.len_chars();
            let line_start = text.line_start(text.line_of_char(self.caret));
            self.push_undo(text);
            let _ = crate::editing::indent(text, self.caret..self.caret, true);
            let removed = before - text.len_chars();
            if removed > 0 {
                let caret = self.caret.saturating_sub(removed).max(line_start);
                self.set_caret(caret, text.len_chars());
                out.edited = true;
            }
            return;
        }
        self.push_undo(text);
        let (lo, hi) = self.selection(text.len_chars());
        // Which lines the selection touches, worked out before the edit.
        let first = text.line_of_char(lo);
        let last_line = text.line_of_char(hi);
        let _ = crate::editing::indent(text, lo..hi, outdent);
        // The selection is deliberately re-established over those same lines
        // rather than collapsed. Collapsing it would leave nothing selected, so
        // Shift+Tab straight after Tab would do nothing at all — and undoing an
        // indent by outdenting is the reason both keys exist.
        let after = text.len_chars();
        let from = text.line_start(first).min(after);
        // The end is the end of the *last* line, measured from that line's own
        // start. Measuring from the first line's start would shorten the
        // selection by however much came before it, which leaves the tail of the
        // last line out of it and Shift+Tab then outdents only part of it.
        let to = text.line_end(last_line).min(after);
        self.anchor = from;
        self.caret = to.max(from);
        out.edited = true;
    }

    /// Splits the line at the caret, keeping the indent of the code on it and
    /// stepping in after an opening bracket.
    fn enter(&mut self, text: &mut Buffer, out: &mut Outcome) {
        self.push_undo(text);
        // Asked before the newline goes in: afterwards the caret is on a fresh
        // empty line, and an empty line has no indentation to copy from.
        let indent = crate::editing::indent_for_new_line(text, self.caret);
        // Between a bracket and its closer — the state right after typing `{` — Enter
        // opens the block: the closer drops to a line of its own at the line's
        // indent, and the caret waits indented between the two.
        let between = !self.has_selection(text.len_chars())
            && self
                .caret
                .checked_sub(1)
                .and_then(|i| text.char_at(i))
                .zip(text.char_at(self.caret))
                .is_some_and(|(open, close)| {
                    crate::editing::PAIRS
                        .iter()
                        .any(|&(o, c)| o == open && c == close && o != c)
                });
        if between {
            let line = text.line_str(text.line_of_char(self.caret));
            let base: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let inner = format!("\n{indent}");
            let closer = format!("\n{base}");
            let caret = self.caret;
            text.insert(caret, &format!("{inner}{closer}"));
            self.set_caret(caret + inner.chars().count(), text.len_chars());
            out.edited = true;
            return;
        }
        self.insert_undoed(text, &format!("\n{indent}"), out);
    }

    // ---- caret movement ---------------------------------------------------

    /// Selects the whole line `at` is on, newline included.
    ///
    /// The newline goes with it, because that is what a reader means by "this
    /// line" when they are about to delete it — a selection that stops before the
    /// newline leaves an empty line behind, so Delete appears to do nothing.
    pub fn select_line_at(&mut self, text: &Buffer, at: usize) {
        let line = text.line_of_char(at);
        let start = text.line_start(line);
        let end = text.line_end(line);
        // The newline goes with the line, because that is what a reader means by
        // "this line" when they are about to delete it: a selection that stops
        // before the newline leaves an empty line behind, so Delete appears to do
        // nothing. The last line of a document has no newline after it, and the
        // `min` is what stops that case inventing one.
        self.anchor = start;
        self.caret = (end + 1).min(text.len_chars()).max(start);
    }

    /// The caret's line and column, one-based, as a status bar shows them.
    ///
    /// The column counts characters, not bytes and not display columns, so a line
    /// of multi-byte text reports the character the caret is between — which is
    /// what every other editor reports and the only thing a reader can act on.
    pub fn line_and_column(&self, text: &Buffer) -> (usize, usize) {
        let line = text.line_of_char(self.caret);
        let column = self.caret - text.line_start(line) + 1;
        (line + 1, column)
    }

    /// Scrolls sideways so the caret's column is on screen, if it is not.
    ///
    /// Without this, a line longer than the pane is only readable up to its own
    /// width: the caret walks off the right edge, `End` puts it somewhere the
    /// reader cannot see, and there is no way to bring it back. The wrap toggle
    /// is the other answer to the same problem, and both are needed — wrapping
    /// changes how a file reads, and scrolling is how a file is navigated.
    ///
    /// `visible` is how much room the text has and `room` how far it can be
    /// pushed. Both are measured rather than derived here, because they are
    /// questions about the pane and the shaped rows respectively.
    fn keep_caret_in_view_x(
        &mut self,
        galley: &egui::Galley,
        m: &Metrics,
        visible: f32,
        room: f32,
    ) {
        if room <= 0.0 {
            self.scroll_x = 0.0;
            return;
        }
        let Some(local) = m.window.to_local(self.caret) else {
            return;
        };
        let row = m.window.row_of_local(local, &m.rows);
        if row >= galley.rows.len() || row >= m.rows.len() {
            return;
        }
        // From the row's own glyphs rather than from `pos_from_cursor`, which
        // panics on a cursor at the end of the galley. That panic is what a guard
        // here used to be for, and the guard cost more than it saved: the end of
        // the last line *is* one past the last glyph, so `local >= len` was
        // refusing to scroll sideways for a caret at the end of the document, and
        // pressing End on a long line did nothing.
        let x = Self::x_of_boundary(galley, row, local.saturating_sub(m.rows[row].chars.0));
        // A margin, so the caret is not left pressed against the edge where half
        // of it is cut off.
        const MARGIN: f32 = 40.0;
        if x - self.scroll_x > visible - MARGIN {
            self.scroll_x = x - visible + MARGIN;
        } else if x - self.scroll_x < MARGIN {
            self.scroll_x = x - MARGIN;
        }
        self.scroll_x = self.scroll_x.clamp(0.0, room);
    }

    /// Scrolls so that the caret's line is on screen, if it is not already.
    ///
    /// Called after every caret move rather than being worked into each one,
    /// because the alternative is a scroll rule per key and they always disagree
    /// eventually: moving down off the bottom edge is the one that gets missed,
    /// and it is the one that matters most.
    fn scroll_to_caret(&mut self, text: &Buffer) {
        if self.wrapping {
            // Lines are different heights here, so where the caret is on screen has
            // to be measured; the frame does it, before anything is shaped.
            self.reveal_caret = true;
            return;
        }
        let line = text.line_of_char(self.caret);
        let h = self.scroll_h.max(1.0);
        // Against the pane as it is, not against the rows that are shaped: the window
        // shapes a row or two more than fit, so that scrolling never shows a gap, and a
        // caret on one of those is below the bottom edge of what can be seen.
        let view = self.view_h.max(h);
        let top = line as f32 * h;
        if top < self.scroll_y {
            self.scroll_to(top);
        } else if top + h > self.scroll_y + view {
            self.scroll_to(top + h - view);
        }
    }

    /// How wide a character really is when laid out, in points.
    ///
    /// Measured from laid-out text, not asked of the font: a row of two hundred and
    /// fifty-six of them is shaped once and divided. The font's own figure differs
    /// from what the layout places by a small fraction of a point, and over a hundred
    /// thousand characters a small fraction is hundreds of points — enough to put the
    /// caret off the end of the pane. Remembered per character.
    fn measure(&mut self, ui: &egui::Ui, font: &egui::FontId, c: char) -> f32 {
        if let Some(&w) = self.wide_w.get(&c) {
            return w;
        }
        let row: String = std::iter::repeat_n(c, 256).collect();
        let galley = ui.fonts_mut(|f| f.layout_no_wrap(row, font.clone(), egui::Color32::WHITE));
        let w = galley.size().x / 256.0;
        self.wide_w.insert(c, w);
        w
    }

    /// How wide a character is, in points, without shaping the line it is in.
    ///
    /// Every plain ASCII character is as wide as `0` in the monospace font, a tab is
    /// measured, and anything else is measured once and remembered.
    fn char_w(&mut self, ui: &egui::Ui, font: &egui::FontId, adv: f32, c: char) -> f32 {
        if c == '\t' {
            self.measure(ui, font, '\t')
        } else if c.is_ascii() {
            adv
        } else {
            self.measure(ui, font, c)
        }
    }

    /// The width of the first `cols` characters of `line`.
    fn prefix_width(
        &mut self,
        ui: &egui::Ui,
        text: &Buffer,
        font: &egui::FontId,
        line: usize,
        cols: usize,
    ) -> f32 {
        let adv = self.measure(ui, font, '0');
        let start = text.line_start(line);
        let mut x = 0.0;
        let mut chars = text.chars_at(start);
        for _ in 0..cols.min(text.line_len(line)) {
            x += self.char_w(ui, font, adv, chars.next().unwrap_or(' '));
        }
        x
    }

    /// The whole width of a long line, from the cache when nothing has changed.
    fn long_line_width(
        &mut self,
        ui: &egui::Ui,
        text: &Buffer,
        font: &egui::FontId,
        line: usize,
    ) -> f32 {
        let len = text.line_len(line);
        if let Some((l, n, g, w)) = self.long_width
            && l == line
            && n == len
            && g == self.edit_gen
        {
            return w;
        }
        let w = self.prefix_width(ui, text, font, line, len);
        self.long_width = Some((line, len, self.edit_gen, w));
        w
    }

    /// Which part of a long line to shape: the columns `c0..c1`, and how far in from
    /// the line's own left edge the first of them sits.
    ///
    /// Wide enough on both sides of the pane that scrolling a little needs no new
    /// slice, and rounded to [`SLICE_STEP`] columns so that it is the same slice, and
    /// so a shaped-text cache hit, for most of a scroll.
    fn long_slice(
        &mut self,
        ui: &egui::Ui,
        text: &Buffer,
        font: &egui::FontId,
        line: usize,
        view_w: f32,
    ) -> LongSlice {
        let start = text.line_start(line);
        let len = text.line_len(line);
        let adv = self.measure(ui, font, '0');
        let want_left = (self.scroll_x - SLICE_MARGIN).max(0.0);
        let (mut x, mut col) = (0.0f32, 0usize);
        let mut chars = text.chars_at(start);
        while col < len {
            let w = self.char_w(ui, font, adv, chars.next().unwrap_or(' '));
            if x + w > want_left {
                break;
            }
            x += w;
            col += 1;
        }
        let c0 = col / SLICE_STEP * SLICE_STEP;
        let lead = self.prefix_width(ui, text, font, line, c0);
        let want_right = self.scroll_x + view_w + SLICE_MARGIN;
        let (mut x, mut end) = (lead, c0);
        let mut chars = text.chars_at(start + c0);
        while end < len && x < want_right {
            x += self.char_w(ui, font, adv, chars.next().unwrap_or(' '));
            end += 1;
        }
        let c1 = end.div_ceil(SLICE_STEP).saturating_mul(SLICE_STEP).min(len);
        let full = self.long_line_width(ui, text, font, line);
        LongSlice { c0, c1, lead, full }
    }

    /// Scrolls sideways to the caret when it is on a long line, before that line is
    /// shaped, since where the slice is depends on where the view is.
    ///
    /// The same rule as [`Editor::keep_caret_in_view_x`], measured from character
    /// widths instead of shaped glyphs, because the caret may be in a stretch of the
    /// line that is not shaped at all.
    fn follow_caret_on_long_line(
        &mut self,
        ui: &egui::Ui,
        text: &Buffer,
        font: &egui::FontId,
        view_w: f32,
    ) {
        let line = text.line_of_char(self.caret);
        if text.line_len(line) <= LONG_LINE || self.followed_caret == Some(self.caret) {
            return;
        }
        self.followed_caret = Some(self.caret);
        let col = self.caret - text.line_start(line);
        let x = self.prefix_width(ui, text, font, line, col);
        let room = (self.long_line_width(ui, text, font, line) + END_PAD - view_w).max(0.0);
        const MARGIN: f32 = 40.0;
        if x - self.scroll_x > view_w - MARGIN {
            self.scroll_x = x - view_w + MARGIN;
        } else if x - self.scroll_x < MARGIN {
            self.scroll_x = x - MARGIN;
        }
        self.scroll_x = self.scroll_x.clamp(0.0, room);
    }

    /// Moves the caret up or down by visual rows, the way every editor does.
    ///
    /// Three things the plain column arithmetic of [`Editor::move_line`] gets wrong,
    /// and this gets right:
    ///
    /// - **Where you were.** The caret's horizontal position is remembered across a
    ///   run of vertical moves, so passing through a short line does not leave it
    ///   stranded at that line's end on the way to a long one. The memory lasts only
    ///   while the caret stays where the last vertical move put it: a click, a
    ///   typed letter or a Left forget it, because each of those moves the caret.
    /// - **Where that is on screen.** The position is in points, not characters. A
    ///   tab is four characters wide and a letter is one, so the same character
    ///   column is a different place, and the caret would jump sideways moving
    ///   between an indented line and a plain one.
    /// - **What a row is.** With wrap on, a long line is several rows, and Down
    ///   moves to the next of them rather than clear over the rest of the line.
    ///
    /// Up from the first row goes to the start of the document and Down from the
    /// last goes to its end, which is what a reader who has run out of lines expects.
    pub fn move_vertical(
        &mut self,
        ui: &egui::Ui,
        text: &Buffer,
        rows: i32,
        extend: bool,
        wrap: bool,
    ) {
        if rows == 0 {
            return;
        }
        let last_line = text.lines().saturating_sub(1);
        let mut line = text.line_of_char(self.caret);
        let mut galley = self.layout_line(ui, text, line, wrap);
        let (mut starts, mut row) = Self::row_starts(&galley, text.line_len(line), {
            self.caret - text.line_start(line)
        });
        let goal = match self.goal {
            Some((at, x)) if at == self.caret => x,
            _ => {
                let col = self.caret - text.line_start(line);
                Self::x_of_boundary(&galley, row, col.saturating_sub(starts[row]))
            }
        };
        let mut at_edge = false;
        for _ in 0..rows.unsigned_abs() {
            if rows > 0 {
                if row + 1 < starts.len() - 1 {
                    row += 1;
                } else if line < last_line {
                    line += 1;
                    galley = self.layout_line(ui, text, line, wrap);
                    starts = Self::row_starts(&galley, text.line_len(line), 0).0;
                    row = 0;
                } else {
                    at_edge = true;
                    break;
                }
            } else if row > 0 {
                row -= 1;
            } else if line > 0 {
                line -= 1;
                galley = self.layout_line(ui, text, line, wrap);
                starts = Self::row_starts(&galley, text.line_len(line), 0).0;
                row = starts.len() - 2;
            } else {
                at_edge = true;
                break;
            }
        }
        let target = if at_edge {
            // Ran out of lines: the start of the first or the end of the last.
            if rows < 0 { 0 } else { text.len_chars() }
        } else {
            let row_len = starts[row + 1] - starts[row];
            let mut col = self.column_in_row(&galley, row, goal).min(row_len);
            // The end of a wrapped row is the start of the next one, so a caret
            // aimed at it belongs on this row's last character instead.
            if row + 1 < starts.len() - 1 && col == row_len && row_len > 0 {
                col -= 1;
            }
            text.line_start(line) + starts[row] + col
        };
        self.place(target, extend);
        self.goal = Some((self.caret, goal));
        self.scroll_to_caret(text);
    }

    /// One logical line laid out on its own, wrapped at the pane's width when wrap
    /// is on. The galley is cached by egui, so asking again is a hash lookup.
    fn layout_line(
        &self,
        ui: &egui::Ui,
        text: &Buffer,
        line: usize,
        wrap: bool,
    ) -> Arc<egui::Galley> {
        let width = if wrap {
            self.wrap_w.max(20.0)
        } else {
            f32::INFINITY
        };
        let job = egui::text::LayoutJob::simple(
            text.line_str(line).into_owned(),
            font(),
            egui::Color32::WHITE,
            width,
        );
        ui.fonts_mut(|f| f.layout_job(job))
    }

    /// The character offset each row of a laid-out line starts at, ending with the
    /// line's length, and which row `col` falls in.
    fn row_starts(galley: &egui::Galley, len: usize, col: usize) -> (Vec<usize>, usize) {
        let mut starts = vec![0usize];
        for placed in &galley.rows {
            let next = (starts[starts.len() - 1] + placed.row.glyphs.len()).min(len);
            starts.push(next);
        }
        if starts.len() == 1 {
            starts.push(0);
        }
        // Whatever the rows did not account for belongs to the last of them.
        let end = starts.len() - 1;
        starts[end] = len;
        let row = (0..end)
            .rev()
            .find(|&r| starts[r] <= col)
            .unwrap_or(0)
            .min(end - 1);
        // A caret on the boundary between two rows is at the start of the second.
        (starts, row)
    }

    /// Moves the caret by whole lines, keeping the column where it can.
    ///
    /// Column arithmetic on logical lines, without a layout. The keyboard uses
    /// [`Editor::move_vertical`] instead; this stays for callers that have no `Ui`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn move_line(&mut self, text: &Buffer, delta: i32, extend: bool, _rows_visible: usize) {
        let line = text.line_of_char(self.caret);
        let col = self.caret.saturating_sub(text.line_start(line));
        let last = text.lines().saturating_sub(1);
        let target = (line as i64 + delta as i64).clamp(0, last as i64) as usize;
        let target_start = text.line_start(target);
        let len = text.line_len(target);
        self.place(target_start + col.min(len), extend);
        self.scroll_to_caret(text);
    }

    /// Moves the caret to a line and column, extending the selection if asked.
    pub fn place(&mut self, caret: usize, extend: bool) {
        if extend {
            self.caret = caret;
        } else {
            self.caret = caret;
            self.anchor = caret;
        }
    }

    /// Start of the caret's line.
    pub fn line_start_char(&self, text: &Buffer) -> usize {
        text.line_start(text.line_of_char(self.caret))
    }

    /// End of the caret's line, excluding its newline.
    pub fn line_end_char(&mut self, text: &Buffer) -> usize {
        text.line_end(text.line_of_char(self.caret))
    }

    /// The first character of the line, or of the next one, for double
    /// clicking a word.
    pub fn word_at(&self, text: &Buffer, at: usize) -> (usize, usize) {
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '/' || c == '.';
        let Some(c) = text.char_at(at) else {
            return (at, at);
        };
        if !is_word(c) {
            return (at, at + 1);
        }
        let mut lo = at;
        while lo > 0 && text.char_at(lo - 1).is_some_and(is_word) {
            lo -= 1;
        }
        let mut hi = at;
        while text.char_at(hi).is_some_and(is_word) {
            hi += 1;
        }
        (lo, hi)
    }
}

// ---- the widget ------------------------------------------------------------

/// Identifies the editor to egui's focus system.
///
/// A fixed id rather than one derived from the surrounding `Ui`, because the
/// same editor is drawn in two different places depending on whether focus mode
/// is on: in the right-hand panel, or in the middle of the window. With a
/// per-`Ui` id those would be two different editors, and switching between them
/// would drop the caret and the scroll position.
pub const ID: &str = "rhumb-editor";

/// Where the editor's own glyphs come from.
fn font() -> egui::FontId {
    crate::theme::mono_font(crate::theme::fs::MONO)
}

/// The characters between two character indices.
///
/// Bounds are clamped rather than trusted, because the selection can be a
/// character out of date by the time it is read: the buffer is edited in place
/// and a stale index must not panic.
fn slice_chars(text: &Buffer, lo: usize, hi: usize) -> String {
    text.slice(lo, hi).into_owned()
}

/// One visual row: the characters it covers, and the line it came from.
///
/// Carrying the line here is what saves the gutter a lookup per row. The obvious
/// way to find which line a row belongs to is to convert the row's first
/// character back into a line number, but that conversion counts characters from
/// the top of the document, so drawing the gutter would cost O(file) every frame.
/// The rows are walked alongside the lines anyway, so the answer is already in
/// hand by the time it is needed.
#[derive(Clone, Copy, Debug)]
pub struct RowSpan {
    /// Window-local character range this row covers.
    pub chars: (usize, usize),
    /// The document line this row is part of.
    pub line: usize,
}

/// Character range of each *visual* row, in window-local indices.
///
/// One entry per row the layout produced, which is not the same as one per line
/// once soft wrap is on and a long line occupies several rows.
///
/// The rows do not say which line they belong to, and their glyph counts cannot
/// be simply added up to find out: a `Row` holds one glyph per character, but
/// the newline that ends a line is consumed as the row break rather than kept.
/// Summing glyph counts therefore leaves every row boundary one character short,
/// and since the shortfalls accumulate the twentieth row is twenty characters
/// out — enough for the caret, the gutter numbers and the click hit-test to all
/// be answering about a different line. So the lines that were appended are
/// walked alongside the rows, and each line's newline is accounted for as the
/// row moves on to the next one.
fn row_ranges(
    rows: &[egui::epaint::text::PlacedRow],
    line_chars: &[usize],
    first_line: usize,
    base: usize,
) -> Vec<RowSpan> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0usize;
    let mut col = 0usize;
    let mut at = base;
    for placed in rows {
        // A row's glyphs are its visible characters. The newline that ends a line
        // is not among them: the layout consumes it as the row break.
        //
        // Capped at what the line has left, because a glyph is not a character and
        // the two come apart in exactly the case that matters. A combining mark is
        // laid out as glyphs of its own in addition to the base character it
        // decorates, so a line of `◌́0◌́` is three characters and five glyphs.
        // Taking the glyph count as the character count gave that line a range
        // five characters long, put every line after it two characters out of
        // step, and made the window claim two characters more than the document
        // holds — so a click at the end of the last line addressed a position
        // past the end of the buffer, and the next keystroke sliced the text with
        // it. A row cannot cover more characters than its line has left, whatever
        // it is made of.
        let room = line_chars.get(i).copied().unwrap_or(0).saturating_sub(col);
        let n = placed.row.glyphs.len().min(room);
        if i >= line_chars.len() {
            // Past the end of the shaped text: the trailing empty row a final
            // newline leaves, and anything after it. No characters, so nothing to
            // hit-test, but it is still a row to draw and to leave a caret on.
            //
            // Its line is deliberately one past the last. It is the position after
            // the final newline, which belongs to no line of the document, and
            // giving it the last line's number would put two rows on one line
            // where the layout has put a break between them.
            out.push(RowSpan {
                chars: (at, at),
                line: first_line + i,
            });
            continue;
        }
        out.push(RowSpan {
            chars: (at, at + n),
            line: first_line + i,
        });
        at += n;
        col += n;
        // Step over the newline and on to the next line once this one is used up.
        //
        // A blank line is what makes this fiddly. It has no characters, so the
        // test "has this row used up the line" is true before the row has done
        // anything, and stepping straight over it would give it no row at all.
        // Every line after it would then be drawn against the wrong range — and
        // clicking on the empty line would find no row, putting the caret at the
        // bottom of the window. A row that carries no characters while the line it
        // belongs to is also empty *is* that line's row, and the line is finished
        // afterwards.
        //
        // One row per line and no more. The layout is asked for a job that is the
        // visible lines and their newlines, and it answers with one row per
        // newline-terminated line including the empty ones — so consecutive blank
        // lines arrive here already spaced out, one row each. An earlier version
        // had a loop to hand out rows to blank lines it thought the layout had
        // not made, and it invented a row for every blank line after the first:
        // five rows for a document of three blank lines, two of them at the same
        // characters, so a click between them could land on either.
        if col >= line_chars[i] {
            at += 1;
            col = 0;
            i += 1;
        }
    }
    out
}

/// Width of the caret, in points. Thin, but wide enough to be easy to see.
const CARET_W: f32 = 1.5;

/// The shades of grey each token class is drawn in.
///
/// Kept as its own type so the tokenizer stays free of any colour type, and so
/// the mapping from class to shade is one readable table rather than a colour
/// chosen at each call site.
#[derive(Clone, Copy, Debug)]
struct Palette {
    text: egui::Color32,
    dim: egui::Color32,
    ghost: egui::Color32,
    accent: egui::Color32,
}

impl Palette {
    fn from_theme() -> Palette {
        Palette {
            text: crate::theme::c::TEXT,
            dim: crate::theme::c::TEXT_DIM,
            ghost: crate::theme::c::TEXT_GHOST,
            accent: crate::theme::c::ACCENT,
        }
    }

    /// Comments are the dimmest thing on screen so that prose recedes behind the
    /// code, and keywords are the brightest so that structure reads first.
    fn of(&self, token: Token) -> egui::Color32 {
        match token {
            Token::Plain => self.text,
            Token::Str | Token::Number => self.dim,
            Token::Comment => self.ghost,
            Token::Keyword => self.accent,
        }
    }
}

/// Everything the paint pass needs, worked out once per frame.
///
/// Grouped rather than passed as eight separate arguments because every one of
/// these is derived from the same shaped galley, and threading them by hand is
/// how the caret ends up drawn against a different row list than the selection.
struct Metrics {
    font: egui::FontId,
    /// Top-left of the text, which is the gutter's width in from the rect.
    origin: egui::Pos2,
    rect: egui::Rect,
    /// Width of the line-number column, zero when there is no gutter.
    gutter: f32,
    /// One entry per row the layout produced.
    rows: Vec<RowSpan>,
    /// Top of each row, relative to the galley's own origin.
    ///
    /// Read back from the galley rather than worked out from the font. The
    /// galley is what actually placed the glyphs, so it is the only thing that
    /// can say where a row really is; re-deriving it from a row height and
    /// multiplying by the row index is how the caret ends up a couple of pixels
    /// lower than the text on every line, and eighty pixels lower by the
    /// fortieth.
    row_y: Vec<f32>,
    /// Height of each row, for the same reason.
    row_h: Vec<f32>,
    window: Window,
    pal: Palette,
}

/// Width of the scrollbar strip along the right edge, and the smallest a thumb
/// may be drawn.
///
/// The width is generous for a monochrome interface where the bar is the only
/// thing that says a file is longer than the window — a hairline would be easy
/// to miss and impossible to grab.
const SCROLLBAR_W: f32 = 11.0;
const THUMB_MIN_H: f32 = 26.0;
/// How thick the thumb is at rest and how thick it grows to, in points. The grown bar
/// reaches further left than the strip that is set aside for it, over the edge of the
/// text, which is what lets it be a wide target without taking width from the text.
const THUMB_THIN: f32 = 3.0;
const THUMB_WIDE: f32 = 14.0;
/// How close the pointer has to come, in points to the left of the bar, before it starts
/// growing to meet it.
const BAR_NEAR: f32 = 48.0;
/// How far the pressable area of the grown bar reaches to the left of the strip.
const BAR_REACH: f32 = 10.0;

/// Height of the find bar's row, which is taken off the top of the pane.
const FIND_H: f32 = 30.0;

impl Metrics {
    /// Top of row `row`, in window coordinates.
    fn row_top(&self, row: usize) -> f32 {
        self.origin.y + self.row_y.get(row).copied().unwrap_or(0.0)
    }

    /// Height of row `row`.
    fn row_height(&self, row: usize) -> f32 {
        self.row_h.get(row).copied().unwrap_or(1.0).max(1.0)
    }
}

impl Editor {
    /// Draws the editor over `rect`, editing `text` in place.
    ///
    /// The cost of a frame is proportional to what is on screen, not to the
    /// size of the file. That is the entire point of this module: the widget
    /// it replaces re-shaped every line on every frame, which is why a
    /// 6,000-line file cost more than a whole 60 fps frame while idle.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        text: &mut Buffer,
        opts: &Options,
    ) -> Outcome {
        let mut out = Outcome::default();
        // A search on another thread wakes the window through this when it answers,
        // and its answer is taken in before anything reads the hits.
        self.find.ctx = Some(ui.ctx().clone());
        if let Some(jump) = self.find.poll()
            && jump
        {
            self.show_hit(text);
        }
        // The index is asked about the buffer twice per frame: here, and again after
        // the keyboard. Both are free when nothing has changed — one integer
        // compare each — and the second is what lets the keyboard run *before* the
        // shaping rather than after it, which is the difference between drawing the
        // text as it is after this keystroke and drawing it as it was before. A
        // frame of lag on every character typed is the whole of what "typing feels
        // janky" turned out to be.
        self.ensure_index(text);
        // The pane as the caller gave it, before the find bar took its strip off
        // the top. The bar is drawn over this and the text over what is left.
        let full_rect = rect;
        let font = font();
        // The galley gives every row this height, so it is what decides how
        // many rows to shape. Drawing still reads its geometry back from the
        // galley rather than trusting this number.
        // The distance between two rows as they are drawn, which is not the font's own
        // figure: the layout snaps a row to a whole pixel, so a font that is 14.54
        // points a row is drawn 15 apart. Everything that counts rows against the pane
        // has to use what is drawn, or the two drift apart a row every few dozen lines
        // and a caret that is "on screen" is below the bottom of it.
        let scroll_h = ui
            .fonts_mut(|f| {
                let two = f.layout_no_wrap(
                    "A
A"
                    .to_owned(),
                    font.clone(),
                    egui::Color32::WHITE,
                );
                match (two.rows.first(), two.rows.get(1)) {
                    (Some(a), Some(b)) => b.pos.y - a.pos.y,
                    _ => f.row_height(&font),
                }
            })
            .max(1.0);
        // Remembered so a scroll position in points can be turned back into a line
        // without measuring the font again, and so the furthest scroll can be worked
        // out from the document rather than from the last frame's row count.
        self.scroll_h = scroll_h;
        // The find bar takes its height off the top of the pane before anything
        // else is measured, because "how many rows fit" is asked of what is left.
        // Measured rather than assumed, so a row can never be half hidden under
        // the bar, which is the one thing a find bar must not do to the text it
        // is searching.
        self.find_h = self.find_bar_h();
        let rect = egui::Rect::from_min_max(
            egui::Pos2::new(rect.left(), rect.top() + self.find_h),
            rect.max,
        );
        // Set before the clamp below, because the clamp is a question about the
        // last *screenful* and that depends on how many rows fit.
        self.view_h = rect.height();
        self.rows_visible = ((rect.height() / scroll_h).ceil() as usize + 1).max(1);

        // ---- the wheel, before anything is shaped ----
        //
        // This is here rather than with the rest of the pointer handling because
        // it decides *which* lines get shaped. Handled after the shaping, the
        // frame would draw the lines the wheel has just moved away from, and the
        // one it is moving towards would arrive a frame late — which is a visible
        // smear behind every flick of the wheel, and the whole reason a
        // smooth-scrolling editor can feel loose.
        self.wrapping = opts.wrap;
        self.wheel_scroll(ui, rect, text);
        self.rescroll(ui, text);

        // One row more than fits, so a line clipped by the bottom edge is drawn
        // rather than popping in as you scroll.
        let pal = Palette::from_theme();
        let gutter = if opts.line_numbers {
            self.gutter(ui, &font, opts)
        } else {
            0.0
        };

        // ---- focus, before anything is painted ----
        // Focus goes through egui's own memory rather than a field, because that
        // is the only thing which keeps typing out of this editor while the
        // pointer and the keyboard are in the search box. A click here takes
        // focus; a press anywhere else gives it up, which is what stops a stray
        // letter from landing in the file behind the file list.
        //
        // Resolved before painting rather than after, because the caret is only
        // drawn while the editor holds focus: deciding it afterwards would draw
        // the caret according to the previous frame, and the first click into an
        // editor would show no caret at all until something else happened.
        let id = egui::Id::new(ID);
        // The text area only, not the whole pane. egui gives a press to one
        // widget, and the editor's own target covers the scrollbar if it is
        // allowed to — so a press on the bar moves the caret instead of the
        // window, and the bar cannot be dragged at all.
        //
        // Measured from this frame's opening geometry rather than the final one,
        // which is not known until the keyboard has been read: the find bar may
        // open on this very frame and take the top off. The two differ by the
        // height of a bar, so the press target is right to within a strip that only
        // matters on the one frame the bar appears, and being right on the other
        // frames is worth more than being right on that one.
        let press_rect = egui::Rect::from_min_max(
            rect.min,
            egui::Pos2::new(
                rect.left() + rect.width() - self.scrollbar_rect(rect).map_or(0.0, |b| b.width()),
                rect.bottom(),
            ),
        );
        let resp = ui.interact(press_rect, id, egui::Sense::click_and_drag());
        // Focus is taken when a drag begins as well as when a click finishes. A person
        // who drags across text in a document they have never clicked in means to select
        // it and then to copy it or type over it, and the drag has to have given the
        // document the keyboard for either to work.
        if resp.clicked_by(egui::PointerButton::Primary)
            || resp.drag_started_by(egui::PointerButton::Primary)
        {
            ui.memory_mut(|m| m.request_focus(id));
        }
        // Tab and the arrow keys have to be claimed *exclusively*, or egui moves the
        // focus somewhere else on the same press. That is the difference between a
        // text field that accepts Tab and one where Tab silently walks off to the next
        // widget: without the filter, Tab both indents and unfocuses, and the indent
        // is never seen.
        //
        // Every frame while the editor has the keyboard, not once when it is clicked:
        // the keyboard is also given to it without a click (a file opened, the window
        // filled, the find bar closed), and a focus that was asked for rather than
        // clicked comes with no filter, so an arrow key then walked the focus away
        // from the text instead of moving the caret.
        // Escape too while there is more than one caret: it is how they are put away,
        // and with one caret it gives the keyboard up, as it always did.
        let claim_escape = !self.extra.is_empty();
        ui.memory_mut(|m| {
            if m.has_focus(id) {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: claim_escape,
                    },
                );
            }
        });
        let mut focused = ui.memory(|m| m.has_focus(id));
        if !focused
            && let Some(pos) = ui.input(|i| i.pointer.press_origin())
            && !press_rect.contains(pos)
        {
            // A press outside gives focus up, so a stray letter cannot land in
            // the file behind the file list. When the press was in a text box
            // that box has already claimed focus and this changes nothing.
            ui.memory_mut(|m| m.surrender_focus(id));
            focused = ui.memory(|m| m.has_focus(id));
        }
        self.focused = focused;
        // Whether the pointer really was clicked, as opposed to egui's idea of a
        // click.
        //
        // `Response::clicked` is true for a press of Space or Enter on a widget that
        // has the keyboard, and this widget is a focusable one that the editor gives
        // the keyboard to. So pressing Enter to split a line reported a click on the
        // editor, and the click is a request to put the caret where the pointer is:
        // press Enter within a moment of clicking and the caret jumped back to the
        // click. `clicked_by` is the same question without the keyboard, and the
        // only difference between the two is exactly the difference that matters
        // here.
        let clicked = resp.clicked_by(egui::PointerButton::Primary);
        if focused {
            self.keyboard(ui, text, opts, &mut out);
        }
        // The find bar's own keys, here rather than at the end of the frame for the
        // same reason the keyboard is here: stepping to a match scrolls the window,
        // and a window that moves after the text has been shaped leaves the frame
        // drawing the lines it is leaving. It runs outside the focus gate on
        // purpose — while the bar is open the *field* has the keyboard, which is the
        // whole point of the bar.
        if self.find.open {
            self.find_keys(ui, text);
        }
        // Whatever the bar learned last frame is acted on here, before the window
        // is measured and the text is shaped.
        // The hits are positions in a buffer that has just been edited, so they are
        // found again before anything reads them. Marked stale on the frame the edit
        // lands and refreshed here, on the same frame, rather than being left to
        // point at text that has moved.
        if out.edited {
            self.find.stale = true;
        }
        if self.find.open && self.find.stale {
            self.find.refresh(text);
        }
        self.find.can_replace = opts.editable;
        if self.find.pending.is_some() {
            self.apply_replace(text, &mut out, opts.editable);
        }
        if self.find.open || self.find.stale {
            self.find_sync(text);
        }
        // The line and character counts are read again here, after the keyboard,
        // because both the clamp below and the shaping measure the document against
        // them. Clamping a window against the counts from before an edit asked "how
        // many lines are there" of the document as it was, so a document that had
        // just shrunk from twenty-one lines to one kept a window starting at line
        // twenty, shaped nothing at all, and drew an empty pane.
        self.ensure_index(text);
        // A drag on the scrollbar moves the window, so it is read here too — after
        // the find bar has been given its keys, which can also move the window, and
        // before the text is shaped. It used to be read immediately before the
        // paint, which drew the thumb where the drag had put it and left the text
        // showing the lines the drag had just scrolled away from, so pressing the
        // track moved the bar and not the page.
        if let Some(bar) = self.scrollbar_rect(rect) {
            self.scrollbar_drag(ui, bar, rect);
        }
        // The caret and the anchor are positions in the buffer, and the buffer is
        // edited in place. A position set before this frame's edit is one
        // character out of date once it lands, and a reader that trusts it goes on
        // to address text that is not there — which is a panic on the next
        // keystroke, not a wrong-looking caret. Clamped once here, at the only
        // point where the buffer's final length is known.
        //
        // The length is the one `ensure_index` has just taken, which the rope keeps
        // as a running count, so this costs nothing however large the file is.
        let len = self.len;
        self.caret = self.caret.min(len);
        self.anchor = self.anchor.min(len);
        // Recorded after the edits rather than before, so the next frame's readers
        // — the status bar, the find bar — are not a keystroke behind.
        // The bar may have opened this frame, and the bar takes its height off the
        // top of the text. Measured again so the text moves down on the frame the
        // bar appears rather than the frame after, which otherwise put a row of
        // letters under an opaque strip for one frame — the one thing a find bar
        // must never do to the text it is searching.
        self.find_h = self.find_bar_h();
        // Worked out again from the pane as the caller gave it, because the bar may
        // have opened or closed while the keyboard was being read. The text has to
        // move down on the frame the bar appears rather than the frame after, or a
        // row of letters sits under the bar for one frame — the one thing a find
        // bar must never do to the text it is searching.
        let rect = egui::Rect::from_min_max(
            egui::Pos2::new(full_rect.left(), full_rect.top() + self.find_h),
            full_rect.max,
        );
        // Set before the clamp below, because the clamp is a question about the
        // last *screenful* and that depends on how many rows fit.
        self.rows_visible = ((rect.height() / scroll_h).ceil() as usize + 1).max(1);
        self.rescroll(ui, text);
        // One row more than fits, so a line clipped by the bottom edge is drawn
        // rather than popping in as you scroll.
        let shaped = self.rows_visible + 1;
        // Worked out before the text is shaped, because it decides how wide the
        // text may be: the bar sits over the text rather than beside it only if
        // the layout is not told about it, and a line scrolled under an opaque
        // bar looks like a rendering fault rather than a scrollbar.
        let bar = self.scrollbar_rect(rect);
        let text_w = rect.width() - bar.map_or(0.0, |b| b.width());
        // The area the text occupies, which is everything but the scrollbar. Used
        // as the clip, as the click target and as the hit test, so that the three
        // cannot disagree about where the text is.
        let clip = egui::Rect::from_min_max(
            rect.min,
            egui::Pos2::new(rect.left() + text_w, rect.bottom()),
        );
        // Worked out after the keyboard, because a keystroke can move the
        // window and the window decides what is shaped.
        let first = self.top_line;
        let base_char = text.line_start(first);

        // ---- shape only what is on screen ----
        self.wrap_w = (text_w - gutter).max(20.0);
        let wrap = if opts.wrap {
            egui::text::TextWrapping::wrap_at_width((text_w - gutter).max(20.0))
        } else {
            egui::text::TextWrapping::no_max_width()
        };
        let mut job = egui::text::LayoutJob {
            wrap,
            ..Default::default()
        };
        // The character length of each line as it is appended, so that the rows
        // the layout produces can be matched back to the lines they came from.
        // With soft wrap on there are more rows than lines, and nothing in a row
        // says which line it is part of.
        let mut line_chars: Vec<usize> = Vec::with_capacity(shaped);
        // The first line *after* the window. Whether the last shaped line gets a
        // newline follows from this, so it is worked out once here rather than
        // re-asked inside the loop.
        let end_line = (first + shaped).min(text.lines());
        // An edit changes what the lines below it start in, so what was remembered
        // about them is dropped before anything is looked up.
        if let Some(line) = text.take_dirty_line() {
            self.hl.invalidate_from(line);
            self.edit_gen += 1;
        }
        // The state the first shaped line starts in. `None` while a worker is still
        // finding it out, in which case the text is drawn uncoloured for now.
        let mut state = if opts.highlight {
            self.hl.state_at(text, &opts.lang, first, ui.ctx())
        } else {
            None
        };
        // Long lines are shaped in slices, and the stretches left out are recorded so
        // that a position in the shaped text can still be turned into a position in
        // the document.
        let mut gaps: Vec<(usize, usize)> = Vec::new();
        let mut local_len = 0usize;
        let mut long_widest = 0.0f32;
        let view_w = (text_w - gutter).max(1.0);
        if !opts.wrap {
            self.follow_caret_on_long_line(ui, text, &font, view_w);
        }
        for line in first..end_line {
            let len = text.line_len(line);
            let newline_after = line + 1 < end_line;
            if !opts.wrap && len > LONG_LINE {
                let slice = self.long_slice(ui, text, &font, line, view_w);
                long_widest = long_widest.max(slice.full);
                let line_start = text.line_start(line);
                let mut piece_chars = 0usize;
                let colour_it = opts.highlight && len <= markup::MAX_HIGHLIGHT_LINE;
                match state.filter(|_| colour_it) {
                    Some(start) => {
                        // Coloured over the whole line, so a string or comment opened
                        // before the slice still colours what is in it, and then only
                        // the runs that fall inside the slice are laid out.
                        let whole = text.line_str(line);
                        let (runs, next) = tokenize_with(&whole, &opts.lang, start);
                        state = Some(next);
                        let (mut at, mut lead) = (0usize, slice.lead);
                        for (run, token) in runs {
                            let n = run.chars().count();
                            let (from, to) = (at.max(slice.c0), (at + n).min(slice.c1));
                            if from < to {
                                let part: String =
                                    run.chars().skip(from - at).take(to - from).collect();
                                job.append(
                                    &part,
                                    lead,
                                    egui::TextFormat::simple(font.clone(), pal.of(token)),
                                );
                                lead = 0.0;
                                piece_chars += to - from;
                            }
                            at += n;
                        }
                    }
                    None => {
                        let piece = text.slice(line_start + slice.c0, line_start + slice.c1);
                        job.append(
                            &piece,
                            slice.lead,
                            egui::TextFormat::simple(font.clone(), pal.text),
                        );
                        piece_chars = slice.c1 - slice.c0;
                    }
                }
                if slice.c0 > 0 {
                    gaps.push((local_len, slice.c0));
                }
                if slice.c1 < len {
                    gaps.push((local_len + piece_chars, len - slice.c1));
                }
                line_chars.push(piece_chars);
                local_len += piece_chars;
            } else {
                let content = text.line_str(line);
                if let Some(start) =
                    state.filter(|_| opts.highlight && content.len() <= markup::MAX_HIGHLIGHT_LINE)
                {
                    let (runs, next) = tokenize_with(&content, &opts.lang, start);
                    state = Some(next);
                    for (run, token) in runs {
                        job.append(
                            &run,
                            0.0,
                            egui::TextFormat::simple(font.clone(), pal.of(token)),
                        );
                    }
                } else {
                    job.append(
                        &content,
                        0.0,
                        egui::TextFormat::simple(font.clone(), pal.text),
                    );
                }
                line_chars.push(len);
                local_len += len;
            }
            // The newline is what starts the next row in the job, and it is a
            // character like any other as far as the indices go.
            //
            // Appended only when another line follows it *in this job*, which is not
            // the same question as whether the document has a line after it. A
            // window that stops in the middle of a document has more lines below
            // its last one, and appending a newline for a line that was never
            // shaped gave the job a trailing break: the layout answered with one
            // more row than there were lines, that row was attributed to the line
            // after the window — which is not on screen — and everything derived
            // from the rows, including the window's length and how far a click can
            // reach, came out a line too long.
            //
            // The same rule covers the end of the document, where the last line has
            // no newline of its own: a document ending in a newline has an empty
            // line *after* that newline, and that is the one line not terminated.
            if newline_after {
                job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), pal.text));
                local_len += 1;
            }
        }
        let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
        // How tall a line is on average, from the ones just drawn. It is what turns
        // the scrollbar's distance into a line while wrapping, where lines are
        // different heights and only the ones on screen have been measured. Smoothed,
        // so the thumb does not twitch as long and short lines pass through.
        if opts.wrap {
            let drawn = end_line.saturating_sub(first).max(1) as f32;
            let now = (galley.size().y / drawn).max(scroll_h);
            self.line_h_est = if self.line_h_est <= 0.0 {
                now
            } else {
                self.line_h_est * 0.9 + now * 0.1
            };
        }
        // What was shaped, recorded for the tests.
        //
        // A test that wants to know what the editor laid out has to find it among
        // every other piece of text on the frame, and on an empty document the
        // gutter's "1" is the same size and has more characters than the
        // document's nothing, so "the biggest text on screen" is the wrong answer
        // and picks the line number. Recorded here so the identification is exact
        // rather than inferred from geometry.
        #[cfg(test)]
        {
            self.shaped = galley.job.text.clone();
        }
        let rows = row_ranges(&galley.rows, &line_chars, first, 0);
        self.last_rows = rows.clone();
        // Every character the job holds, which is how far a click can reach.
        let at = rows.last().map_or(0, |r| r.chars.1);
        // How far the text can be pushed sideways, worked out before it is
        // positioned: the offset is a shift of the origin, so it costs nothing
        // to apply and has to be known before the origin is built.
        // Lifted by however far into its first row the window is scrolled. This is
        // the sub-line part of `scroll_y`, and it is the reason a wheel can move the
        // text by a few points at a time instead of a whole row: `first` is a whole
        // line because rows and lines are whole things, and the remainder between
        // them is drawn as a shift.
        let sub_line = self.top_off;
        let mut m = Metrics {
            font: font.clone(),
            origin: egui::Pos2::new(
                rect.left() + gutter,
                snap_to_pixel(ui, rect.top() - sub_line),
            ),
            rect,
            gutter,
            window: Window::with_gaps(base_char, at, gaps),
            pal,
            rows,
            // Taken straight from the rows that were produced.
            row_y: galley.rows.iter().map(|r| r.pos.y).collect(),
            row_h: galley.rows.iter().map(|r| r.row.size.y.max(1.0)).collect(),
        };
        let widest = galley
            .rows
            .iter()
            .map(|r| r.row.size.x)
            .fold(0.0f32, f32::max)
            .max(long_widest);
        // A little past the last character, so the caret at the end of the longest line
        // is inside the pane and not against its edge.
        let room = (widest + END_PAD - (text_w - gutter)).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, room);
        // Only when the caret has moved. Done on every frame it would drag the view back
        // to the caret the moment it was scrolled sideways, and nothing could be read
        // that the caret was not next to.
        if self.followed_caret != Some(self.caret) {
            self.keep_caret_in_view_x(&galley, &m, (text_w - gutter).max(1.0), room);
            if m.window.to_local(self.caret).is_some() {
                self.followed_caret = Some(self.caret);
            }
        }
        m.origin.x -= self.scroll_x;
        // Recorded for the tests, once the origin is final.
        //
        // An empty document lays out no galley at all, so there is nothing on the
        // frame to measure the text's left edge from and a test has to be told
        // where the text would have started.
        #[cfg(test)]
        {
            self.origin = m.origin;
        }

        // ---- pointer ----
        // A plain click collapses any selection and puts the caret where the
        // pointer is. The modifiers are read from the press rather than from the
        // release, because a user who holds shift to extend a selection is still
        // holding it at the moment they let go of the button, and the platform
        // reports it either way — but the press is the frame the click is
        // recognised in, so that is where the answer is.
        if clicked && let Some(at) = self.hit(&galley, &m, &resp) {
            if ui.input(|i| i.modifiers.shift) {
                // Extend from wherever the selection already started. The anchor
                // is the end that has not moved, which is why a selection that was
                // built up and then extended keeps growing from its original end
                // rather than jumping.
                self.caret = at;
            } else if ui.input(|i| i.modifiers.alt) {
                // Alt adds a caret where the pointer is, or takes the one that is
                // there away.
                self.toggle_cursor_at(at, text.len_chars());
            } else {
                self.extra.clear();
                self.place(at, false);
            }
            self.blink = 0.0;
            // Clicking is a deliberate act, so it ends the run of typing that
            // undo would otherwise lump together. Without this, moving the caret
            // and typing again is indistinguishable from typing all along, and one
            // undo throws away both.
            self.last_edit = None;
        }
        // Where a drag begins.
        //
        // Set on the *press*, from the point the button went down at, and this is
        // the whole of why dragging selects from where the pointer went down rather
        // than from wherever the caret happened to be. A drag that never becomes a
        // click — and a drag never does, because a pointer that moves past the
        // threshold gives up its claim to being one — has nothing but this to set
        // the anchor with, so leaving the anchor alone here anchored the selection
        // to the caret's last position and the highlight began somewhere the
        // pointer had never been.
        if resp.drag_started()
            && focused
            && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        {
            let at = self.hit_at(&galley, &m, pos);
            // Shift held when the drag began extends the selection that is already
            // there, which is what shift-drag means everywhere. Alt starts a new
            // selection beside the others.
            if ui.input(|i| i.modifiers.shift) {
                self.caret = at;
            } else if ui.input(|i| i.modifiers.alt) {
                self.toggle_cursor_at(at, text.len_chars());
            } else {
                self.extra.clear();
                self.place(at, false);
            }
            self.blink = 0.0;
        }
        if resp.dragged()
            && focused
            && let Some(pos) = resp.interact_pointer_pos()
        {
            // The pointer's own position, not the hover position: once it leaves the
            // pane nothing is hovered, and a drag that only listened to hovers
            // stopped growing the selection the moment the pointer left.
            //
            // Held inside the text vertically, so a pointer above or below the pane
            // selects to the first or last visible row rather than to nothing.
            let inside = egui::Pos2::new(pos.x, pos.y.clamp(rect.top() + 1.0, rect.bottom() - 1.0));
            let at = self.hit_at(&galley, &m, inside);
            // The anchor is left where it was, so a drag grows the selection
            // from the character the drag started on.
            self.caret = at;
            self.blink = 0.0;
            // Past the top or bottom, the window scrolls at a speed that grows with
            // how far out the pointer is, and keeps asking for frames because the
            // pointer standing still outside the pane produces no events to draw.
            let outside = if pos.y < rect.top() {
                pos.y - rect.top()
            } else if pos.y > rect.bottom() {
                pos.y - rect.bottom()
            } else {
                0.0
            };
            if outside != 0.0 {
                self.scroll_pending = (outside * 2.0).clamp(-400.0, 400.0);
                ui.ctx().request_repaint();
            }
        }
        if resp.double_clicked()
            && let Some(at) = self.hit(&galley, &m, &resp)
        {
            let (lo, hi) = self.word_at(text, at);
            self.anchor = lo;
            self.caret = hi;
        }
        // Checked after the double click, so a triple click lands on the line
        // rather than the word: the second click of a triple is also a double
        // click, and without the order the narrower answer would win.
        if resp.triple_clicked()
            && let Some(at) = self.hit(&galley, &m, &resp)
        {
            self.select_line_at(text, at);
        }

        // ---- paint ----
        // Back to front, so each layer covers the one before it: the gutter and
        // the selection sit under the glyphs, and the caret over them.
        //
        // All of it through a painter clipped to the text area, so a long
        // unwrapped line stops at the scrollbar rather than running under it and
        // out the other side.
        let painter = ui.painter().with_clip_rect(clip);
        if opts.line_numbers {
            self.paint_gutter(ui, &painter, &m, text);
        }

        if self.find.open {
            self.paint_hits(&painter, &galley, &m);
        }
        self.paint_selection(&painter, &galley, &m);
        painter.galley(m.origin, galley.clone(), m.pal.text);
        self.paint_caret(&painter, &galley, &m);
        if let Some(bar) = bar {
            self.scrollbar_paint(ui, bar);
        }

        // An edit moves the character index of every hit after it, so the hits have
        // to be found again. Marked here and acted on by `find_sync` on the next
        // frame, because the text it would be found in has not been shaped yet at
        // the point an edit is made.
        if out.edited {
            self.find.stale = true;
        }
        // Turning wrap on makes the sideways offset meaningless, because a wrapped
        // line is never wider than the pane. A different document is handled by
        // `reset`, which rebuilds the whole editor.
        if opts.wrap {
            self.scroll_x = 0.0;
        }
        // The find bar, drawn last and over everything, because it is the only
        // part of the editor that is not the text. It is asked for after the
        // keyboard rather than before, so a key pressed this frame is answered by
        // the same pass that will draw the answer.
        if self.find.open {
            self.find_bar(ui, full_rect, text);
        }
        // Advanced at the very end, so the phase the caret was *painted* with is
        // the phase it was tested against. Advancing before the paint means the
        // first frame after a long pause starts in the hidden half of the cycle
        // and the caret appears to be missing.
        self.blink += ui.input(|i| i.unstable_dt.min(0.5));
        // A caret that blinks needs a frame at each turn of the blink, and a window
        // with nothing else to do needs no others.
        if self.focused {
            let phase = self.blink % 1.0;
            let (mut next, after) = if phase < 0.55 {
                (0.55 - phase, 0.45)
            } else {
                (1.0 - phase, 0.55)
            };
            // Just short of a turn, the frame is left to the one after it: a caret
            // that changes forty milliseconds late is not seen to, and a frame for it
            // would be wasted.
            if next < 0.04 {
                next += after;
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(next));
        }
        out
    }

    /// Enter, Shift+Enter, Escape and F3, while the find bar is open.
    ///
    /// Deliberately outside the "does the editor have the keyboard" gate, because
    /// while the bar is open the *field* has it — which is the whole point of the
    /// bar — and a key handler that only runs when the editor is focused can never
    /// see the key that closes the thing that took the focus away from it.
    ///
    /// The events are read rather than the state, because a key that is held down
    /// sends one press and then a stream of repeats, and a reader who holds Enter
    /// expects to run through the matches rather than to see one of them.
    fn find_keys(&mut self, ui: &egui::Ui, text: &Buffer) {
        let events = ui.input(|i| i.events.clone());
        for ev in &events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };
            let shift = modifiers.shift;
            let ctrl = modifiers.ctrl || modifiers.command;
            // Claimed before being acted on, for the same reason the rest of the
            // editor does it: the app has its own Escape and its own Ctrl+F, and a
            // key that both sides answer is a key that does two things at once.
            if !matches!(key, egui::Key::Escape | egui::Key::Enter | egui::Key::F3)
                && !(ctrl && matches!(key, egui::Key::F | egui::Key::H))
            {
                continue;
            }
            if !ui.input_mut(|i| i.consume_key(*modifiers, *key)) {
                continue;
            }
            match key {
                // Ctrl+F while the bar is already open puts the keyboard back in
                // the field, rather than falling through to the app's *file*
                // search — which is what would otherwise happen, because the field
                // has the focus and the editor's own key pass is gated on having
                // it.
                egui::Key::F if ctrl => {
                    ui.memory_mut(|m| m.request_focus(egui::Id::new("find-needle")));
                }
                egui::Key::H if ctrl => self.open_replace(ui),
                egui::Key::Escape => {
                    self.find.open = false;
                    // Focus handed straight back, because a reader who dismisses
                    // the bar is about to type and a letter going into the file
                    // list instead would be the wrong answer.
                    ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
                }
                // Enter in the replace field replaces the current match and moves to
                // the next, which is what a reader who has typed a replacement and
                // pressed Enter is asking for.
                egui::Key::Enter
                    if !shift && ui.memory(|m| m.has_focus(egui::Id::new("find-replacement"))) =>
                {
                    self.find.pending = Some(false);
                }
                egui::Key::Enter | egui::Key::F3 => {
                    // Enter steps forwards, shift+Enter backwards; F3 is the other
                    // way round, which is the arrangement every editor settled on
                    // and the only one where both of a reader's hands work.
                    let forward = if *key == egui::Key::Enter {
                        !shift
                    } else {
                        shift
                    };
                    if let Some((lo, hi)) = self.find.step(forward) {
                        self.select(lo, hi, text.len_chars());
                        self.scroll_to_caret(text);
                    }
                }
                _ => {}
            }
        }
    }

    /// Draws the find bar across the top of the pane, and takes what it is told.
    ///
    /// A row of its own rather than a floating box over the text, because the text
    /// moves down to make room for it. A find bar that covers the lines it is
    /// about to select is worse than no bar at all: the reader is told the answer
    /// is on line 400 and cannot see line 400.
    fn find_bar(&mut self, ui: &mut egui::Ui, rect: egui::Rect, text: &Buffer) {
        let bar =
            egui::Rect::from_min_max(rect.min, egui::Pos2::new(rect.right(), rect.top() + FIND_H));
        {
            // Scoped, because the painter borrows the `Ui` and the field below
            // needs it mutably. Holding it across the row is what stops the bar
            // from compiling at all.
            let painter = ui.painter();
            let whole = egui::Rect::from_min_max(
                rect.min,
                egui::Pos2::new(rect.right(), rect.top() + self.find_bar_h()),
            );
            painter.rect_filled(whole, egui::CornerRadius::ZERO, crate::theme::c::SEL);
            painter.hline(
                whole.left()..=whole.right(),
                whole.max.y - 0.5,
                egui::Stroke::new(1.0, crate::theme::c::DIVIDER),
            );
        }

        // ---- what to search for ----
        let pad = crate::theme::sp::SM;
        let h = bar.height() - 2.0 * crate::theme::sp::XS;
        let field = egui::Rect::from_min_size(
            egui::Pos2::new(bar.left() + pad, bar.top() + crate::theme::sp::XS),
            egui::vec2((bar.width() * 0.5).max(120.0), h),
        );
        let mut typed = self.find.needle.clone();
        let response = ui
            .new_child(
                egui::UiBuilder::new()
                    .max_rect(field)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            )
            .add(
                egui::TextEdit::singleline(&mut typed)
                    .id(egui::Id::new("find-needle"))
                    .desired_width(field.width() - 2.0 * pad)
                    .hint_text("Find"),
            );
        if response.changed() {
            // Re-found from the first hit rather than kept where it was: a needle one
            // letter longer has a different set of hits, and the old current one is
            // very unlikely to still be in it.
            self.find.needle = typed;
            self.find.current = 0;
            self.find.jump = true;
            self.find.refresh(text);
            // And the first one is selected, because a reader who has typed two
            // letters wants to see the first thing it found without having to press
            // Enter — which is also what makes Enter mean "the next one".
            self.show_hit(text);
        }
        // ---- how many ----
        let count = if self.find.needle.is_empty() {
            String::new()
        } else if self.find.searching {
            "Searching\u{2026}".to_owned()
        } else if self.find.hits.is_empty() {
            "No results".to_owned()
        } else {
            let n = self.find.current + 1;
            let mark = if self.find.capped { "+" } else { "" };
            format!("{n} of {}{mark}", self.find.hits.len())
        };
        let count_g =
            crate::widgets::layout(ui, count, crate::theme::ui_font(crate::theme::fs::SMALL), {
                if self.find.hits.is_empty() && !self.find.needle.is_empty() {
                    crate::theme::c::TEXT_GHOST
                } else {
                    crate::theme::c::TEXT_DIM
                }
            });
        let count_x = field.right() + pad;
        crate::widgets::galley_at(
            ui.painter(),
            egui::Pos2::new(count_x, bar.center().y - count_g.size().y * 0.5),
            &count_g,
            crate::theme::c::TEXT_DIM,
        );

        // ---- the buttons ----
        // Right to left, so the close button is always in the same corner and the
        // rest queue up to its left in a fixed order.
        let btn = h;
        let mut x = bar.right() - pad - btn;
        // A slot to the left of the last one, because the buttons are laid out
        // from the right and each one has to know where it went. The rectangles
        // are kept so a test can find a button by name rather than by guessing
        // where the bar's padding is.
        // Collected locally and handed over at the end, because the buttons need
        // &self to draw and &mut self.find_buttons to be recorded, and a
        // borrow of the editor cannot be both at once.
        let mut slots: Vec<egui::Rect> = Vec::with_capacity(6);
        let mut take = || {
            let r = egui::Rect::from_min_size(
                egui::Pos2::new(x, bar.center().y - btn * 0.5),
                egui::vec2(btn, btn),
            );
            x -= btn;
            slots.push(r);
            r
        };
        let mut step = false;
        let mut forward = true;
        if self.flat_button(ui, take(), "Close") {
            self.find.open = false;
            ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
        }
        if self.flat_button(ui, take(), "Next match (Enter)") {
            step = true;
        }
        if self.flat_button(ui, take(), "Previous match (Shift+Enter)") {
            step = true;
            forward = false;
        }
        if self.flat_button(ui, take(), "Match whole word only") {
            self.find.whole = !self.find.whole;
            self.find.jump = true;
            self.find.refresh(text);
            self.show_hit(text);
        }
        if self.flat_button(ui, take(), "Match case") {
            self.find.case = !self.find.case;
            self.find.jump = true;
            self.find.refresh(text);
            self.show_hit(text);
        }

        let toggle = self.find.can_replace && self.flat_button(ui, take(), "Replace (Ctrl+H)");
        if toggle {
            self.find.replace = !self.find.replace;
            if self.find.replace {
                ui.memory_mut(|m| m.request_focus(egui::Id::new("find-replacement")));
            }
        }

        self.find_buttons = slots;
        if self.find.replace && self.find.can_replace {
            self.replace_row(ui, rect, field);
        }
        // Pressed on one of the arrow buttons.
        if step {
            self.find.step(forward);
            self.show_hit(text);
        }
    }

    /// Height of the find bar: one row, or two while the replace row is showing.
    fn find_bar_h(&self) -> f32 {
        if !self.find.open {
            0.0
        } else if self.find.replace && self.find.can_replace {
            2.0 * FIND_H
        } else {
            FIND_H
        }
    }

    /// Shows the replace row and puts the keyboard in its field.
    fn open_replace(&mut self, ui: &egui::Ui) {
        if self.find.can_replace {
            self.find.replace = true;
            ui.memory_mut(|m| m.request_focus(egui::Id::new("find-replacement")));
        }
    }

    /// The second row of the bar: what to put in, and the two buttons that put it in.
    fn replace_row(&mut self, ui: &mut egui::Ui, rect: egui::Rect, field: egui::Rect) {
        let pad = crate::theme::sp::SM;
        let row = field.translate(egui::vec2(0.0, FIND_H));
        let mut typed = self.find.replacement.clone();
        let response = ui
            .new_child(
                egui::UiBuilder::new()
                    .max_rect(row)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            )
            .add(
                egui::TextEdit::singleline(&mut typed)
                    .id(egui::Id::new("find-replacement"))
                    .desired_width(row.width() - 2.0 * pad)
                    .hint_text("Replace"),
            );
        if response.changed() {
            self.find.replacement = typed;
        }
        let h = row.height();
        let mut x = row.right() + pad;
        for (label, all) in [("Replace", false), ("All", true)] {
            let w = if all { 40.0 } else { 64.0 };
            let r = egui::Rect::from_min_size(egui::Pos2::new(x, row.top()), egui::vec2(w, h));
            x += w + 2.0;
            if r.right() > rect.right() {
                break;
            }
            if self.text_button(ui, r, label) {
                self.find.pending = Some(all);
            }
        }
    }

    /// A small labelled button that lights up under the pointer.
    fn text_button(&self, ui: &egui::Ui, r: egui::Rect, label: &str) -> bool {
        let resp = ui.interact(
            r,
            egui::Id::new(("find-text-btn", label)),
            egui::Sense::click(),
        );
        if resp.hovered() {
            ui.painter()
                .rect_filled(r, egui::CornerRadius::same(3), crate::theme::c::HOVER);
        }
        let colour = if resp.hovered() {
            crate::theme::c::TEXT
        } else {
            crate::theme::c::TEXT_DIM
        };
        let g = crate::widgets::layout(
            ui,
            label.to_owned(),
            crate::theme::ui_font(crate::theme::fs::SMALL),
            colour,
        );
        crate::widgets::galley_at(
            ui.painter(),
            egui::Pos2::new(
                r.center().x - g.size().x * 0.5,
                r.center().y - g.size().y * 0.5,
            ),
            &g,
            colour,
        );
        resp.clicked()
    }

    /// Does the replace the bar asked for: the current match, or every match.
    ///
    /// One undo step either way. `last_edit` is cleared first, because a replace
    /// that landed within the typing burst window would otherwise be folded into
    /// the step before it and one Ctrl+Z would undo both.
    fn apply_replace(&mut self, text: &mut Buffer, out: &mut Outcome, editable: bool) {
        let Some(all) = self.find.pending.take() else {
            return;
        };
        if !editable || self.find.hits.is_empty() {
            return;
        }
        let with = self.find.replacement.clone();
        let with_len = with.chars().count();
        self.last_edit = None;
        self.push_undo(text);
        self.last_edit = None;
        let after = if all {
            // From the last match back to the first, so the character positions of
            // the ones still to do are not moved by the ones already done. Each
            // is an edit of its own in the rope, and one step in the undo history.
            let hits = self.find.hits.clone();
            for &(lo, hi) in hits.iter().rev() {
                text.replace(lo, hi, &with);
            }
            hits.first().map_or(0, |h| h.0) + with_len
        } else {
            let (lo, hi) = self.find.hits[self.find.current.min(self.find.hits.len() - 1)];
            text.replace(lo, hi, &with);
            lo + with_len
        };
        let n = text.len_chars();
        self.set_caret(after, n);
        out.edited = true;
        // Where the next match is looked for from. Kept for a search that has to
        // finish on another thread, whose answer arrives after this call returns.
        if !all {
            self.find.jump = true;
            self.find.after = Some(after);
        }
        self.find.refresh(text);
        // On to the next match after the one just written, so repeated presses walk
        // the document. Anchored to the caret rather than to the old index: the
        // replacement can be longer or shorter, and can contain the needle itself.
        if !all {
            self.find.current = self
                .find
                .hits
                .iter()
                .position(|&(lo, _)| lo >= after)
                .unwrap_or(0);
            self.show_hit(text);
        }
    }

    /// Selects the current match, if there is one, and asks for the window to be
    /// brought to it.
    ///
    /// The division of labour here is the whole of it. The *selection* is drawn in
    /// the same frame it is made, from the rows that are already laid out, so it
    /// can be made while the find bar is being drawn. The *window* cannot: it
    /// decides which lines get shaped, so it has to be settled before the shaping
    /// and not one instruction later. So this marks the need and `find_sync`
    /// answers it at the top of the next frame, which costs one frame of scrolling
    /// and nothing else — and costs nothing at all when the match is already on
    /// screen, which is the common case while stepping through hits.
    fn show_hit(&mut self, text: &Buffer) {
        if let Some((lo, hi)) = self.find.selected() {
            self.select(lo, hi, text.len_chars());
            self.find.needs_scroll = true;
        }
    }

    /// Brings the window to the selection, if something this frame moved it.
    ///
    /// Before the text is shaped, and that is the entire reason it is a separate
    /// function. Everything the find bar does that moves the selection was doing it
    /// while the bar was being drawn, which is after the page has been laid out, so
    /// the bar said "1 of 40" and the caret went to the match and the frame drew
    /// the lines the caret had just left. The bar is a drawing job now and nothing
    /// else.
    fn find_sync(&mut self, text: &Buffer) {
        if self.find.needs_scroll {
            self.find.needs_scroll = false;
            self.scroll_to_caret(text);
        }
    }
    /// A square button that is only visible under the pointer, with the shortcut
    /// it stands for in its tooltip.
    ///
    /// Pictogram rather than a word, because five words do not fit in a row this
    /// height and the words are in the tooltips anyway.
    fn flat_button(&self, ui: &egui::Ui, r: egui::Rect, tip: &str) -> bool {
        let resp = ui.interact(r, egui::Id::new(("find-btn", tip)), egui::Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(r, egui::CornerRadius::same(3), crate::theme::c::HOVER);
        }
        let colour = if resp.hovered() {
            crate::theme::c::TEXT
        } else {
            crate::theme::c::TEXT_DIM
        };
        let c = r.center();
        match tip {
            "Close" => {
                let s = r.width() * 0.3;
                let stroke = egui::Stroke::new(1.4, colour);
                ui.painter().line_segment(
                    [egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)],
                    stroke,
                );
                ui.painter().line_segment(
                    [egui::pos2(c.x + s, c.y - s), egui::pos2(c.x - s, c.y + s)],
                    stroke,
                );
            }
            "Replace (Ctrl+H)" => {
                // An arrow, for "this becomes that".
                let s = r.width() * 0.28;
                let stroke = egui::Stroke::new(1.3, colour);
                let painter = ui.painter();
                painter.line_segment([egui::pos2(c.x - s, c.y), egui::pos2(c.x + s, c.y)], stroke);
                painter.line_segment(
                    [
                        egui::pos2(c.x + s, c.y),
                        egui::pos2(c.x + s * 0.2, c.y - s * 0.7),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        egui::pos2(c.x + s, c.y),
                        egui::pos2(c.x + s * 0.2, c.y + s * 0.7),
                    ],
                    stroke,
                );
            }
            "Next match (Enter)" => {
                self.triangle(ui.painter(), c, colour, 1.0);
            }
            "Previous match (Shift+Enter)" => {
                self.triangle(ui.painter(), c, colour, -1.0);
            }
            "Match whole word only" => {
                // A word in brackets with a caret at its end. Two triangles, the
                // obvious thing to reach for, sit next to the two buttons that are
                // literally two triangles — and a reader who has to hover to tell
                // them apart has already lost more time than the icon saved.
                let s = r.width() * 0.26;
                let stroke = egui::Stroke::new(1.2, colour);
                let left = egui::pos2(c.x - s, c.y - s);
                let right = egui::pos2(c.x + s * 0.7, c.y - s);
                for (x, dir) in [(left.x, 1.0), (right.x, -1.0)] {
                    ui.painter()
                        .line_segment([egui::pos2(x, c.y - s), egui::pos2(x, c.y + s)], stroke);
                    let tip = egui::pos2(x + dir * s * 0.45, c.y - s);
                    ui.painter()
                        .line_segment([egui::pos2(x, c.y - s), tip], stroke);
                    ui.painter()
                        .line_segment([egui::pos2(x, c.y + s), tip], stroke);
                }
            }
            _ => {
                // "Match case": a capital A, which is the letter the option is
                // about rather than a word nobody will read at this size.
                let g = crate::widgets::layout(ui, "A".to_owned(), font(), colour);
                crate::widgets::galley_at(
                    ui.painter(),
                    egui::Pos2::new(c.x - g.size().x * 0.5, c.y - g.size().y * 0.5),
                    &g,
                    colour,
                );
            }
        }
        resp.on_hover_text(tip).clicked()
    }

    /// A small triangle, the only shape a pair of these needs to tell apart.
    fn triangle(&self, painter: &egui::Painter, c: egui::Pos2, colour: egui::Color32, dir: f32) {
        let s = 3.5;
        let x = c.x + dir * 3.0;
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x + dir * s, c.y),
                egui::pos2(x - dir * s, c.y - s),
                egui::pos2(x - dir * s, c.y + s),
            ],
            colour,
            egui::Stroke::NONE,
        ));
    }

    /// Width of the line-number column.
    fn gutter(&self, ui: &egui::Ui, font: &egui::FontId, opts: &Options) -> f32 {
        if !opts.line_numbers {
            return 0.0;
        }
        // Wide enough for the largest number that will ever be drawn, so the
        // text does not shuffle sideways as the count grows a digit.
        let digits = self.lines.to_string().len() as f32;
        let w = ui.fonts_mut(|f| f.glyph_width(font, '0')) * digits;
        w + crate::theme::sp::SM + crate::theme::sp::MD
    }

    /// Which document character the pointer is over, if any.
    ///
    /// Worked out here rather than by asking the galley, because the galley
    /// answers "which character is nearest this point in the whole shaped text"
    /// and that is not the same question. Two cases come out wrong:
    ///
    /// - A click in the left margin of a line. There is no character there, and
    ///   the galley resolves it to whatever is nearest, which for a blank line is
    ///   the end of the *following* line — so clicking an empty line jumps the
    ///   caret past everything after it.
    /// - A click past the end of the last line, which should land on the last
    ///   line rather than at the end of the shaped window.
    ///
    /// So the row is found by height first, and only then is the column resolved
    /// within that one row. That is also what a reader expects: the row they
    /// clicked is the row the caret goes on.
    fn hit(&self, galley: &egui::Galley, m: &Metrics, resp: &egui::Response) -> Option<usize> {
        let pos = resp.hover_pos()?;
        Some(self.hit_at(galley, m, pos))
    }

    /// The document character under a point, row by row.
    fn hit_at(&self, galley: &egui::Galley, m: &Metrics, pos: egui::Pos2) -> usize {
        // The row, from the vertical position alone. Below the last row stays on
        // the last row, which is what a click in the space under the text means.
        let row = self
            .row_at_height(m, pos.y)
            .min(m.rows.len().saturating_sub(1));
        let span = m.rows[row];
        // Within the row: the nearest character boundary. A row with no characters
        // of its own — a blank line, or a wrapped line's empty tail — puts the
        // caret at its start, which is the only position it has.
        let local = if span.chars.1 <= span.chars.0 {
            span.chars.0
        } else {
            let len = span.chars.1 - span.chars.0;
            let mut col = self.column_in_row(galley, row, pos.x - m.origin.x);
            // The end of a wrapped row is the start of the next one, so a click to
            // the right of it would put the caret at the start of the row below.
            // The reader clicked at the end of *this* row: after its last letter,
            // in front of the space the line broke on.
            let continues = m
                .rows
                .get(row + 1)
                .is_some_and(|next| next.line == span.line);
            if continues && col >= len {
                col = len - 1;
            }
            span.chars.0 + col
        };
        // The galley speaks window-local indices; the caret and the document do
        // not. This is the one place the two meet.
        m.window.to_document(local)
    }

    /// How many characters into row `row` a horizontal position falls.
    ///
    /// Walks that one row's own glyphs rather than asking the galley to resolve
    /// the point against the whole shaped text. The galley answers "which
    /// character is nearest this point", which is a different question: it looks
    /// for a row as well, and when the position sits exactly on a row boundary
    /// it may answer about the row above — whose width is nothing — so a click
    /// anywhere on a line came back as a column into the line above it, and a
    /// click past the end of a line came back as a column somewhere inside it
    /// rather than at its end.
    fn column_in_row(&self, galley: &egui::Galley, row: usize, x: f32) -> usize {
        let Some(placed) = galley.rows.get(row) else {
            return 0;
        };
        let glyphs = &placed.row.glyphs;
        // Every boundary in the row is a candidate, and the nearest one to the
        // pointer wins. Both edges of every glyph are considered, so a click in
        // the left half of a character puts the caret before it and one in the
        // right half puts it after — which is what a reader expects, and what
        // makes a click land under the pointer rather than always one to its left.
        //
        // The row's own width decides the answer, so a click in the empty space
        // to the right of a line comes out as the end of *that* line. Reading the
        // glyph positions rather than accumulating advances does the same for
        // free, and stays right if the layout ever leaves a gap between them.
        let mut best = 0usize;
        let mut best_d = f32::MAX;
        for (i, glyph) in glyphs.iter().enumerate() {
            let start = glyph.pos.x;
            let end = glyphs
                .get(i + 1)
                .map_or(start + glyph.advance_width, |next| next.pos.x);
            for (boundary, distance) in [(i, (x - start).abs()), (i + 1, (x - end).abs())] {
                // `<=`, so a tie goes to the later boundary. A zero-width glyph — a
                // combining mark — starts and ends at the same x, and the earlier of
                // the two would put a click at the end of a line just short of it.
                if distance <= best_d {
                    best_d = distance;
                    best = boundary;
                }
            }
        }
        best
    }

    /// Which row a vertical position falls in, clamped to the rows there are.
    fn row_at_height(&self, m: &Metrics, y: f32) -> usize {
        let mut row = 0usize;
        while row + 1 < m.rows.len() && y >= m.row_top(row + 1) {
            row += 1;
        }
        row
    }

    fn paint_gutter(&self, ui: &egui::Ui, painter: &egui::Painter, m: &Metrics, text: &Buffer) {
        painter.rect_filled(
            egui::Rect::from_min_size(m.rect.min, egui::vec2(m.gutter, m.rect.height())),
            egui::CornerRadius::ZERO,
            crate::theme::c::CODE_BG,
        );
        painter.vline(
            m.rect.left() + m.gutter - 0.5,
            m.rect.top()..=m.rect.bottom(),
            egui::Stroke::new(1.0, crate::theme::c::DIVIDER),
        );
        // Each row already knows which line it is part of, so there is no
        // character-to-line conversion here at all. That matters: the conversion
        // counts characters from the top of the document, which would make
        // drawing the gutter cost O(file) on every frame.
        //
        // Rows arrive in order, so a number is needed only when the line
        // changes, which keeps a wrapped line from being numbered down the
        // gutter with no set allocated to remember what came before.
        let mut last_line: Option<usize> = None;
        // The layout emits one more row than there are lines, for the position
        // after the document's final newline. It is a real row — the caret can sit
        // on it and it is where pressing Enter at the end of a file goes — but it
        // is not a line, and numbering it would put a number in the gutter for a
        // line the document does not have.
        //
        // So the last line that may be numbered is the last one with anything on
        // it, and whether that is the last line of all depends on how the document
        // ends. A document ending in a newline has an empty line after that
        // newline, and that is the line that does not exist as far as a reader is
        // concerned: there is nothing to scroll to there and nothing to put a
        // caret in. Asking the line index alone cannot tell the two apart, because
        // it counts that empty line, so the document's own last character decides.
        let last_real = text
            .lines()
            .saturating_sub(usize::from(text.ends_with_newline()))
            .saturating_sub(1);
        for (row, span) in m.rows.iter().enumerate() {
            let y = m.row_top(row);
            let h = m.row_height(row);
            if y + h < m.rect.top() || y > m.rect.bottom() {
                continue;
            }
            if last_line == Some(span.line) || span.line > last_real {
                continue;
            }
            last_line = Some(span.line);
            let n = (span.line + 1).to_string();
            let g = crate::widgets::layout(ui, n, m.font.clone(), m.pal.ghost);
            crate::widgets::galley_at(
                painter,
                egui::Pos2::new(
                    m.rect.left() + m.gutter - crate::theme::sp::SM - g.size().x,
                    snap_to_pixel(ui, y),
                ),
                &g,
                m.pal.ghost,
            );
        }
    }

    /// Paints a wash behind every hit, and a stronger one behind the current one.
    ///
    /// Under the glyphs rather than over them, so a match is marked without the
    /// text becoming harder to read — which is the whole point of a match, since
    /// the reader is reading it. The current hit is left to the ordinary
    /// selection, so it is marked in exactly the same way as any other selected
    /// text and the reader does not have to learn a second vocabulary.
    fn paint_hits(&self, painter: &egui::Painter, galley: &egui::Galley, m: &Metrics) {
        let current = self.find.selected();
        // Half the strength of a real selection, so a match is a hint and the one
        // being read is unmistakably the subject. Built rather than derived from
        // the selection colour, because there is no alpha helper to derive it
        // with and a match is a different thing from a selection.
        let sel = crate::theme::c::SEL;
        let wash = egui::Color32::from_rgba_premultiplied(sel.r(), sel.g(), sel.b(), 90);
        for (i, &(lo, hi)) in self.find.hits.iter().enumerate() {
            if Some((lo, hi)) == current {
                continue;
            }
            let (Some(a), Some(b)) = (m.window.to_local(lo), m.window.to_local(hi)) else {
                continue;
            };
            for (row, span) in m.rows.iter().enumerate() {
                let (start, end) = span.chars;
                if end <= a || start >= b {
                    continue;
                }
                painter.rect_filled(
                    self.wash_rect(galley, m, row, a.max(start), b.min(end)),
                    egui::CornerRadius::ZERO,
                    wash,
                );
                let _ = i;
            }
        }
    }

    /// Where the boundary between two characters of row `row` is, as an x offset
    /// from the text's left edge.
    ///
    /// The inverse of [`Self::column_in_row`], and for the same reason it does not
    /// ask the galley: `pos_from_cursor` takes an index into the whole shaped text
    /// and answers with whichever row that index lands in, so a boundary sitting
    /// exactly on a wrap point comes back as the *end of the row above* — the
    /// same index, one row up, at an entirely different place on the screen.
    ///
    /// That is not a rounding difference. Painting a selection from those answers
    /// put the wash at the right edge of the row above and nothing at all on the
    /// row below, so dragging across a wrapped line left the wrapped part
    /// unhighlighted, and a caret on a wrap point was drawn at the end of the line
    /// above rather than at the start of the row it is on.
    fn x_of_boundary(galley: &egui::Galley, row: usize, col: usize) -> f32 {
        let Some(placed) = galley.rows.get(row) else {
            return 0.0;
        };
        let glyphs = &placed.row.glyphs;
        if glyphs.is_empty() {
            // A row with no glyphs of its own — a blank line, or the empty tail
            // of a wrapped one — is as wide as nothing at all, which is the only
            // position it has.
            return 0.0;
        }
        if col >= glyphs.len() {
            // The boundary after the last glyph is that glyph's own left plus its
            // advance: there is no next glyph to read a position from, and the
            // next row's left edge is the wrong answer for the same reason as
            // above.
            let last = glyphs[glyphs.len() - 1];
            return last.pos.x + last.advance_width;
        }
        glyphs[col].pos.x
    }

    /// The rectangle to wash for the characters `[from, to)` of one row.
    ///
    /// `from` and `to` are window-local character indices, the same units the row
    /// spans are in.
    fn wash_rect(
        &self,
        galley: &egui::Galley,
        m: &Metrics,
        row: usize,
        from: usize,
        to: usize,
    ) -> egui::Rect {
        let start = m.rows[row].chars.0;
        let x0 = Self::x_of_boundary(galley, row, from.saturating_sub(start));
        let x1 = Self::x_of_boundary(galley, row, to.saturating_sub(start));
        let y = m.row_top(row);
        egui::Rect::from_min_max(
            egui::Pos2::new(m.origin.x + x0.min(x1), y),
            egui::Pos2::new(m.origin.x + x0.max(x1), y + m.row_height(row)),
        )
    }

    fn paint_selection(&self, painter: &egui::Painter, galley: &egui::Galley, m: &Metrics) {
        self.paint_range(painter, galley, m, self.anchor, self.caret);
        for c in &self.extra {
            self.paint_range(painter, galley, m, c.anchor, c.caret);
        }
    }

    /// The wash over one selection.
    fn paint_range(
        &self,
        painter: &egui::Painter,
        galley: &egui::Galley,
        m: &Metrics,
        anchor: usize,
        caret: usize,
    ) {
        if anchor == caret {
            return;
        }
        let end = m.window.base() + m.window.doc_len();
        let (lo, hi) = (anchor.min(caret).min(end), anchor.max(caret).min(end));
        // Clamped to the window, so a selection that starts above it or ends below it
        // is drawn to the edge instead of not at all.
        let (a, b) = (m.window.to_local_clamped(lo), m.window.to_local_clamped(hi));
        if a == b {
            return;
        }
        for (row, span) in m.rows.iter().enumerate() {
            let (start, end) = span.chars;
            if end <= a || start >= b {
                continue;
            }
            // Clipped to this row, so a selection spanning several lines is
            // drawn as a run on each of them rather than one long box.
            painter.rect_filled(
                self.wash_rect(galley, m, row, a.max(start), b.min(end)),
                egui::CornerRadius::ZERO,
                crate::theme::c::SEL,
            );
        }
    }

    fn paint_caret(&self, painter: &egui::Painter, galley: &egui::Galley, m: &Metrics) {
        // Blinks by hiding itself, rather than by being drawn at reduced alpha,
        // so the caret keeps the same colour it had when it was on.
        if !self.focused || (self.blink % 1.0) >= 0.55 {
            return;
        }
        self.paint_caret_at(painter, galley, m, self.caret);
        for c in &self.extra {
            self.paint_caret_at(painter, galley, m, c.caret);
        }
    }

    fn paint_caret_at(
        &self,
        painter: &egui::Painter,
        galley: &egui::Galley,
        m: &Metrics,
        caret: usize,
    ) {
        let Some(local) = m.window.to_local(caret) else {
            return;
        };
        let row = m.window.row_of_local(local, &m.rows);
        if row >= galley.rows.len() {
            return;
        }
        // Through the row's own glyphs, for the same reason the washes are: a
        // caret on a wrap point has to be drawn at the start of the row it is on,
        // not at the end of the row above, and only this mapping knows which row
        // that is.
        let x = Self::x_of_boundary(galley, row, local.saturating_sub(m.rows[row].chars.0));
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::Pos2::new(m.origin.x + x, m.row_top(row)),
                egui::vec2(CARET_W, m.row_height(row)),
            ),
            egui::CornerRadius::ZERO,
            crate::theme::c::ACCENT,
        );
    }

    /// Keys, when the editor has the pointer's attention.
    fn keyboard(&mut self, ui: &egui::Ui, text: &mut Buffer, opts: &Options, out: &mut Outcome) {
        // The events are collected first because handling one needs `&mut self`
        // and the input borrow is immutable.
        let events = ui.input(|i| i.events.clone());
        for ev in &events {
            match ev {
                egui::Event::Text(t) => {
                    if !opts.editable {
                        continue;
                    }
                    // Typed at every caret.
                    self.each(text, false, |e, tx| e.type_text(tx, t, out));
                    self.scroll_to_caret(text);
                }
                // The clipboard shortcuts never arrive as key presses: the
                // window layer turns them into these three events instead, so
                // the text goes to the system clipboard rather than ours. With
                // the editor focused it owns all three, and says so.
                egui::Event::Copy => {
                    out.clipboard = true;
                    self.copy(ui, text, false, out);
                }
                egui::Event::Cut => {
                    out.clipboard = true;
                    self.copy(ui, text, true, out);
                }
                egui::Event::Paste(s) => {
                    out.clipboard = true;
                    if opts.editable {
                        self.paste_each(text, s, out);
                        self.blink = 0.0;
                        self.scroll_to_caret(text);
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => self.key(ui, *key, *modifiers, text, opts, out),
                _ => {}
            }
        }
    }

    /// One keystroke's worth of typing, at the caret this is run for.
    fn type_text(&mut self, text: &mut Buffer, t: &str, out: &mut Outcome) {
        // Typing the closer of a pair we just opened steps past it rather than
        // inserting a second one. The buffer already holds the closer, so the whole
        // keystroke is a caret move.
        let single: Vec<char> = t.chars().filter(|c| !c.is_control()).collect();
        // Only when what was typed *is* the closer. Without that check, any character
        // typed between a freshly opened pair — the `a` in `(a)` — was taken for the
        // closer and swallowed, and the caret jumped past the bracket, so nothing
        // could be typed inside one.
        if single.len() == 1 && crate::editing::types_over_closer(text, self.caret, single[0]) {
            // Bounded by the document, because the closer this steps over has to exist
            // and `should_skip_closer` only proves the character *after* the caret is
            // one — which is the last position in the document, where stepping over
            // would leave the caret one past the end.
            let end = text.len_chars();
            // Both ends of the selection move. Moving only the caret left the closer it
            // had just stepped over *selected*, and the next thing typed replaced it.
            let extra = std::mem::take(&mut self.extra);
            self.set_caret((self.caret + 1).min(end), end);
            self.extra = extra;
            self.blink = 0.0;
            return;
        }
        if single.is_empty() {
            return;
        }
        // Judged on the *last* character, not on the length of the event. A keyboard
        // sends one character per event, but an input method, a paste and a macro can
        // deliver several at once, and `f(` arriving together should still close the
        // bracket.
        let last = single.last().copied().unwrap_or(' ');
        let ins: String = single.into_iter().collect();
        self.insert(text, &ins, out);
        // Opening a bracket also puts its partner in, with the caret between the two.
        if crate::editing::is_opener(last)
            && let Some(next) = crate::editing::auto_close(text, self.caret)
        {
            self.caret = next;
        }
        self.blink = 0.0;
    }

    /// Takes a line out of the document and puts the caret where it was, for a cut with
    /// nothing selected.
    fn cut_line(&mut self, text: &mut Buffer, out: &mut Outcome) {
        let n = text.len_chars();
        let (lo, hi) = self.selection(n);
        let (_, _, range) = Self::line_range(text, lo, hi);
        let (a, b) = (range.start.min(n), range.end.min(n));
        if a >= b {
            return;
        }
        self.push_undo(text);
        text.remove(a, b);
        let extra = std::mem::take(&mut self.extra);
        self.set_caret(a, text.len_chars());
        self.extra = extra;
        out.edited = true;
    }

    /// Puts the selection on the system clipboard, and cuts it if asked.
    fn copy(&mut self, ui: &egui::Ui, text: &mut Buffer, cut: bool, out: &mut Outcome) {
        if !self.extra.is_empty() {
            // Several carets: what each has selected, one to a line, or if none has
            // anything selected, the line each is on.
            let s = self.copy_text(text);
            if s.is_empty() {
                return;
            }
            ui.ctx().copy_text(s);
            if cut {
                let any = !self.selected_texts(text).is_empty();
                self.each(text, !any, |e, t| {
                    if !any {
                        e.cut_line(t, out);
                    } else if e.has_selection(t.len_chars()) {
                        e.delete(t, false, out);
                    }
                });
            }
            return;
        }
        if !self.has_selection(text.len_chars()) {
            // Nothing selected copies, or cuts, the line the caret is on, newline and
            // all — which is what a reader who presses the key with the caret in a
            // line wants, and what every code editor does.
            let n = text.len_chars();
            let (lo, hi) = self.selection(n);
            let (_, _, range) = Self::line_range(text, lo, hi);
            let (a, b) = (range.start.min(n), range.end.min(n));
            if a >= b {
                return;
            }
            ui.ctx().copy_text(slice_chars(text, a, b));
            if cut {
                self.cut_line(text, out);
            }
            return;
        }
        let (lo, hi) = self.selection(text.len_chars());
        ui.ctx().copy_text(slice_chars(text, lo, hi));
        if cut {
            self.delete(text, false, out);
        }
    }

    fn key(
        &mut self,
        ui: &egui::Ui,
        key: egui::Key,
        m: egui::Modifiers,
        text: &mut Buffer,
        opts: &Options,
        out: &mut Outcome,
    ) {
        let ctrl = m.ctrl || m.command;
        // The app owns save, open and new, and every Alt chord. They are left
        // alone deliberately: the editor runs before the app's key pass, so
        // touching one here would silently break the other.
        //
        // Ctrl+F is the exception, and it used to be in that list. The app's
        // Ctrl+F is a *file* search, which is the right thing when the file list
        // has the keyboard and the wrong thing when a file is open and being read:
        // every editor gives the open document first refusal, and the file list
        // gets Ctrl+F back the moment the document is closed. Claiming the key
        // here is what does that, because `consume_key` below is what the app's
        // own pass finds nothing left to claim.
        // Alt is the app's: Alt+Left and Alt+Right walk back and forward through folders.
        // Alt+Up and Alt+Down are the exception, because inside a document they are how
        // a line is moved, and the app's own use of them (up a folder) is not what
        // anybody pressing them with the caret in a file means.
        let alt_for_the_app = m.alt && !matches!(key, egui::Key::ArrowUp | egui::Key::ArrowDown);
        // Ctrl+Tab and Ctrl+PageUp/PageDown change tab, which the app does. The editor used to
        // take them, Ctrl+Tab as an indent, so with the cursor in a file there was no way to
        // go to the next tab from the keyboard.
        let tab_keys = ctrl
            && matches!(
                key,
                egui::Key::Tab | egui::Key::PageUp | egui::Key::PageDown
            );
        if alt_for_the_app
            || tab_keys
            || (ctrl && matches!(key, egui::Key::S | egui::Key::O | egui::Key::N))
        {
            out.handled_elsewhere = true;
            return;
        }
        // Claim the key before acting on it. `consume_key` is what stops the
        // app's own handler from also acting on the same press in the same
        // frame, and it is why the editor can own Delete and Ctrl+A without
        // the file list hearing about them.
        if !ui.input_mut(|i| i.consume_key(m, key)) {
            return;
        }
        self.blink = 0.0;
        // Commands that are about several carets, or that run for each of them.
        if self.multi_key(ui, key, m, text, opts, out) {
            return;
        }
        self.key_single(ui, key, m, text, opts, out);
    }

    /// A key, as it acts at one caret.
    fn key_single(
        &mut self,
        ui: &egui::Ui,
        key: egui::Key,
        m: egui::Modifiers,
        text: &mut Buffer,
        opts: &Options,
        out: &mut Outcome,
    ) {
        let ctrl = m.ctrl || m.command;
        let shift = m.shift;
        let editable = opts.editable;
        let page = self.rows_visible as i32;
        match key {
            // ---- the find bar ----
            // Enter and Escape are handled in `find_keys`, which does not require
            // the editor to have the keyboard, because while the bar is open the
            // field has it. Only Ctrl+F reaches this far, and it opens the bar.
            egui::Key::Enter | egui::Key::Escape | egui::Key::F3 if self.find.open => {
                // Reached only when the editor has focus *and* the bar is open,
                // which means the reader clicked back into the text. The step or
                // the close has already happened in `find_keys` this frame, so
                // there is nothing to do here — but something has to be here, or
                // Enter would insert a line break as well.
            }
            egui::Key::H if ctrl && opts.editable => {
                let n = text.len_chars();
                let (lo, hi) = self.selection(n);
                self.find
                    .open_with((lo != hi).then(|| slice_chars(text, lo, hi)));
                self.find.refresh(text);
                self.open_replace(ui);
            }
            egui::Key::F if ctrl => {
                let n = text.len_chars();
                let (lo, hi) = self.selection(n);
                self.find
                    .open_with((lo != hi).then(|| slice_chars(text, lo, hi)));
                self.find.jump = true;
                self.find.refresh(text);
                if let Some((lo, hi)) = self.find.selected() {
                    self.select(lo, hi, n);
                }
                // The bar takes the keyboard. `request_focus` on the field is
                // what stops the next letter typed going into the file.
                ui.memory_mut(|m| m.request_focus(egui::Id::new("find-needle")));
            }
            // ---- moving and editing ----
            egui::Key::ArrowLeft => self.move_char(text, -1, shift, ctrl),
            egui::Key::ArrowRight => self.move_char(text, 1, shift, ctrl),
            // Ctrl with a horizontal arrow moves by word; with a vertical one it
            // moves the line itself, as everywhere else.
            egui::Key::ArrowUp if ctrl || m.alt => self.raise(text, false, out),
            egui::Key::ArrowDown if ctrl || m.alt => self.raise(text, true, out),
            egui::Key::ArrowUp => self.move_vertical(ui, text, -1, shift, opts.wrap),
            egui::Key::ArrowDown => self.move_vertical(ui, text, 1, shift, opts.wrap),
            // Ctrl with Home or End goes to the ends of the document, as it does
            // in every other editor. Without the modifier it is the ends of the
            // line.
            egui::Key::Home | egui::Key::End if ctrl => {
                let end = key == egui::Key::End;
                let at = if end { text.len_chars() } else { 0 };
                self.place(at, shift);
                self.scroll_to_caret(text);
            }
            egui::Key::Home if opts.wrap => self.goto_row_edge(ui, text, true, shift),
            egui::Key::End if opts.wrap => self.goto_row_edge(ui, text, false, shift),
            egui::Key::Home => self.goto_line_start(text, shift),
            egui::Key::End => self.goto_line_edge(text, false, shift),
            egui::Key::PageUp => self.move_vertical(ui, text, -page, shift, opts.wrap),
            egui::Key::PageDown => self.move_vertical(ui, text, page, shift, opts.wrap),
            egui::Key::Backspace if editable && ctrl => self.delete_word(text, true, out),
            egui::Key::Backspace if editable => self.delete(text, true, out),
            egui::Key::Delete if editable && ctrl => self.delete_word(text, false, out),
            egui::Key::Delete if editable => self.delete(text, false, out),
            // Delete the line, and duplicate it. Both are line commands, so both
            // act on every line the selection touches rather than on a
            // character range.
            egui::Key::K if ctrl && shift && editable => self.delete_lines(text, out),
            egui::Key::D if ctrl && shift && editable => self.duplicate_lines(text, out),
            // Ctrl+Enter opens a line below the current one and moves onto it, with
            // the caret wherever it was on the line: nothing has to be at the end.
            egui::Key::Enter if editable && ctrl && !shift => {
                let end = self.line_end_char(text);
                self.set_caret(end, text.len_chars());
                self.enter(text, out);
            }
            egui::Key::Enter if editable => self.enter(text, out),
            egui::Key::Tab if editable => self.tab(text, shift, out),
            egui::Key::Slash if ctrl && editable => {
                let (lo, hi) = self.selection(text.len_chars());
                // Pushed before the change, so undo puts back the line as it was
                // rather than as it is about to be.
                self.push_undo(text);
                let (first, last) = (text.line_of_char(lo), text.line_of_char(hi));
                let had_selection = lo != hi;
                let caret = crate::editing::toggle_comment(text, lo..hi, opts.comment);
                if had_selection {
                    // The same lines stay selected, so pressing the key again
                    // uncomments all of them and not just the one the caret is on.
                    let n = text.len_chars();
                    self.anchor = text.line_start(first).min(n);
                    self.caret = text.line_end(last.min(text.lines() - 1)).min(n);
                } else {
                    self.place(caret.unwrap_or(self.caret), false);
                }
                out.edited = true;
            }
            // Clamped, because `select` takes a range and a caret at the very end
            // is a real position: a document ending in a newline has one more
            // character index than it has characters, and selecting to the count
            // would put the caret one past it.
            egui::Key::A if ctrl => {
                let n = text.len_chars();
                self.select(0, n, n);
                // Clamped again immediately, because the anchor and the caret are
                // only as fresh as the last edit and a paste or a deletion since
                // then leaves both pointing past the end. Every reader of them
                // would then be addressing text that is not there.
                self.caret = self.caret.min(n);
                self.anchor = self.anchor.min(n);
            }
            // Ctrl+Z is undo. Redo is Ctrl+Y, or Ctrl+Shift+Z on a keyboard
            // that has the two stacked.
            egui::Key::Z | egui::Key::Y if ctrl => {
                let undo = key == egui::Key::Z && !shift;
                if self.undo_redo(text, undo) {
                    out.edited = true;
                }
            }
            _ => {}
        }
    }

    /// Moves the caret's line up or down, carrying whole lines with it.
    fn raise(&mut self, text: &mut Buffer, down: bool, out: &mut Outcome) {
        let line = text.line_of_char(self.caret);
        let target = if down {
            line + 1
        } else {
            line.saturating_sub(1)
        };
        if target == line || target >= text.lines() {
            return;
        }
        self.push_undo(text);
        // The two lines are adjacent, so this is one replace rather than two
        // removes and two inserts, each of which would invalidate the positions
        // the next one needs. Both lines' own text is taken without its newline,
        // and the newline is put back in the middle.
        let (a, b) = (text.line_start(line), text.line_end(line));
        let (a2, b2) = (text.line_start(target), text.line_end(target));
        let (mine, theirs) = (
            text.slice(a, b).into_owned(),
            text.slice(a2, b2).into_owned(),
        );
        // The caret hops exactly one line, so it moves by the length of the line
        // it lands on plus that line's newline.
        let hop = theirs.chars().count() as i64 + 1;
        let (start, end, swap) = if down {
            (a, b2, format!("{theirs}\n{mine}"))
        } else {
            (a2, b, format!("{mine}\n{theirs}"))
        };
        text.replace(start, end, &swap);
        let over = if down { hop } else { -hop };
        self.place((self.caret as i64 + over).max(0) as usize, false);
        out.edited = true;
    }

    /// The highest line the window can start on.
    ///
    /// The last *screenful*, not the last *line*. Scrolling to the end has to
    /// leave the end of the document at the bottom of the view, which means
    /// starting a screenful earlier; clamping to the last line instead leaves the
    /// reader looking at a screenful of nothing above the end of the file, and
    /// pushes the scrollbar's thumb off the end of its own track.
    fn max_top_line(&self) -> usize {
        let last = self.lines.saturating_sub(1);
        last.saturating_sub(self.rows_visible.saturating_sub(1))
    }

    /// How far the window can scroll down, in lines. Zero when the whole
    /// document fits, which is also the condition for showing a scrollbar at all.
    fn scroll_room(&self) -> usize {
        if self.wrapping {
            (self.max_scroll_y() / self.est_h()).ceil() as usize
        } else {
            self.max_top_line()
        }
    }

    /// The wheel, while the pointer is over the pane.
    ///
    /// Separate from the rest of the pointer handling, and called before anything
    /// is shaped, because it decides *which* lines are shaped. Called after the
    /// shaping, a frame would draw the lines the wheel has just moved away from
    /// and the lines it is moving towards would arrive a frame late — a visible
    /// smear behind every flick of the wheel.
    ///
    /// In points rather than in whole lines, which is the whole difference between
    /// this feeling like a scrollbar and feeling like a staircase. A window that
    /// jumps a line at a time cannot be smooth: a trackpad reports a fraction of a
    /// line per frame, and rounding each of those to a whole line either throws
    /// the movement away or jumps a line at a time, and both of those are what
    /// "not smooth" looks like. The file list beside this editor scrolls the same
    /// way and feels the same way, and it does it by being handed a position in
    /// points and drawing from it.
    fn wheel_scroll(&mut self, ui: &egui::Ui, rect: egui::Rect, text: &Buffer) {
        let over = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| rect.contains(p));
        if over {
            // Shift with the wheel scrolls sideways, which is the one gesture that
            // does it everywhere. Kept out of the vertical path so a trackpad swipe
            // with shift held does not do both.
            // egui has already turned Shift with the wheel, and a trackpad's sideways
            // swipe, into a horizontal delta; the editor takes it as it comes and does
            // not ask whether Shift is down, which is not something egui reliably says.
            let side = ui.input(|i| i.smooth_scroll_delta.x);
            if side != 0.0 {
                self.scroll_x = (self.scroll_x - side).max(0.0);
            }
            // The wheel's own events rather than egui's smoothed total of them.
            //
            // egui eases each notch out over about ten frames, and a wheel that keeps
            // turning gives it a new notch every few frames, so the speed rises and
            // falls in a repeating ripple — fast on the frame after a notch, slower
            // on the next two — which reads as stutter however fine the frames are.
            // Taking the notches and easing towards where they add up to, at one
            // steady rate, is what makes a spinning wheel move at a steady speed.
            //
            // Read before `input` is opened: the context's lock is not re-entrant, and
            // asking it for an option from inside the closure waits on itself forever.
            let line = ui.ctx().options(|o| o.input_options.line_scroll_speed);
            let view_h = self.view_h;
            let (notch_points, touch_points) = ui.input(|i| {
                let (mut notch, mut touch) = (0.0f32, 0.0f32);
                for event in &i.events {
                    if let egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                        ..
                    } = event
                    {
                        // Shift makes it a sideways scroll, handled above, and Ctrl
                        // makes it a zoom; neither moves the text up or down.
                        if modifiers.shift || modifiers.command {
                            continue;
                        }
                        match unit {
                            egui::MouseWheelUnit::Line => notch += delta.y * line,
                            egui::MouseWheelUnit::Page => notch += delta.y * view_h,
                            // A trackpad reports the finger's own movement, in points,
                            // and easing that adds a lag the finger can feel.
                            egui::MouseWheelUnit::Point => touch += delta.y,
                        }
                    }
                }
                (notch, touch)
            });
            // Subtracted, not added. egui names its own sign: a positive wheel delta
            // is scrolling *up*, and it is what the platform reports for a wheel
            // rotated away from the user, so it moves the window towards the top of
            // the document. Adding it instead turns the wheel the wrong way.
            self.scroll_pending -= notch_points;
            if touch_points != 0.0 {
                self.scroll_pending = 0.0;
                self.scroll_by(ui, text, -touch_points);
            }
        }
        self.glide(ui, text);
    }

    /// Covers one frame's share of what the wheel has asked for.
    ///
    /// An exponential approach with a fixed time constant, so the speed depends on
    /// how much is left to go and on nothing else: a steady stream of notches gives
    /// a steady speed, and a single one glides to a stop. Frames are asked for until
    /// it arrives, because nothing else would draw them. Hitting either end of the
    /// document drops whatever was left, so a wheel spun against the end does not
    /// wind up a debt that unwinds the moment it turns back.
    fn glide(&mut self, ui: &egui::Ui, text: &Buffer) {
        if self.scroll_pending.abs() < f32::EPSILON {
            return;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
        let step = if self.scroll_pending.abs() < 0.25 {
            self.scroll_pending
        } else {
            self.scroll_pending * (1.0 - (-dt / WHEEL_EASE).exp())
        };
        let moved = self.scroll_by(ui, text, step);
        if (moved - step).abs() > 0.01 {
            self.scroll_pending = 0.0;
        } else {
            self.scroll_pending -= step;
        }
        if self.scroll_pending.abs() >= f32::EPSILON {
            ui.ctx().request_repaint();
        }
    }

    /// Moves the window by `dy` points of what is actually drawn, down positive, and
    /// returns how far it really went — less than asked at either end.
    ///
    /// Without wrapping every line is one row and this is a plain sum. With it, the
    /// window walks over the lines' own heights, so a wrapped line five rows tall is
    /// crossed at the same speed as a short one instead of being skipped in a single
    /// lurch, which is what a scroll measured in lines does the moment a line wraps.
    fn scroll_by(&mut self, ui: &egui::Ui, text: &Buffer, dy: f32) -> f32 {
        if !self.wrapping {
            let before = self.scroll_y;
            self.set_scroll(before + dy);
            return self.scroll_y - before;
        }
        let moved = self.walk(ui, text, dy);
        if dy > 0.0 {
            moved - self.clamp_bottom(ui, text)
        } else {
            moved
        }
    }

    /// The raw move behind [`Editor::scroll_by`], with no limit at the bottom.
    fn walk(&mut self, ui: &egui::Ui, text: &Buffer, dy: f32) -> f32 {
        let last = text.lines().saturating_sub(1);
        let (mut moved, mut left) = (0.0f32, dy);
        while left > 0.0 {
            let h = self.line_height(ui, text, self.top_line);
            let room = h - self.top_off;
            if left < room {
                self.top_off += left;
                moved += left;
                left = 0.0;
            } else if self.top_line < last {
                moved += room;
                left -= room;
                self.top_line += 1;
                self.top_off = 0.0;
            } else {
                // Off the end of the last line; `clamp_bottom` brings it back.
                self.top_off = h;
                moved += room;
                break;
            }
        }
        while left < 0.0 {
            if -left <= self.top_off {
                self.top_off += left;
                moved += left;
                left = 0.0;
            } else if self.top_line > 0 {
                moved -= self.top_off;
                left += self.top_off;
                self.top_line -= 1;
                self.top_off = self.line_height(ui, text, self.top_line);
            } else {
                moved -= self.top_off;
                self.top_off = 0.0;
                break;
            }
        }
        self.sync_scroll_y();
        moved
    }

    /// Pulls the window back up if it has been scrolled past the point where the end
    /// of the document is at the bottom of the pane. Returns how far it moved.
    ///
    /// Measured from the window down, and only as far as it takes to fill the pane,
    /// so it costs the lines that are on screen and no more.
    fn clamp_bottom(&mut self, ui: &egui::Ui, text: &Buffer) -> f32 {
        let last = text.lines().saturating_sub(1);
        let mut below = -self.top_off;
        let mut line = self.top_line;
        loop {
            below += self.line_height(ui, text, line);
            if below >= self.view_h {
                return 0.0;
            }
            if line >= last {
                break;
            }
            line += 1;
        }
        // The document ends short of the bottom of the pane: back up by the gap.
        let gap = self.view_h - below;
        -self.walk(ui, text, -gap)
    }

    /// The height of one line as drawn: its rows times the row height. One row
    /// unless the text wraps.
    fn line_height(&self, ui: &egui::Ui, text: &Buffer, line: usize) -> f32 {
        if !self.wrapping {
            return self.scroll_h;
        }
        // Taken from the laid-out line rather than worked out as rows times the
        // font's row height: the two differ by a fraction of a point per row, and
        // over a tall wrapped line that is a visible hop each time the window
        // crosses one.
        self.layout_line(ui, text, line, true)
            .size()
            .y
            .max(self.scroll_h)
    }

    /// The height a line is taken to be when it has not been measured.
    fn est_h(&self) -> f32 {
        if self.wrapping {
            self.line_h_est.max(self.scroll_h)
        } else {
            self.scroll_h.max(1.0)
        }
    }

    /// Brings `scroll_y` in line with where the window is anchored.
    fn sync_scroll_y(&mut self) {
        self.scroll_y = self.top_line as f32 * self.est_h() + self.top_off;
    }

    /// Runs once a frame, after whatever has moved the window and before the text is
    /// shaped: holds the window inside the document and, while wrapping, brings the
    /// caret into view if something has asked.
    fn rescroll(&mut self, ui: &egui::Ui, text: &Buffer) {
        if !self.wrapping {
            self.reveal_caret = false;
            self.set_scroll(self.scroll_y);
            return;
        }
        let last = text.lines().saturating_sub(1);
        self.top_line = self.top_line.min(last);
        // A line can be shorter than when the offset into it was set — the pane was
        // widened, or the line was edited — so the offset is folded back into the
        // lines it now spans.
        loop {
            let h = self.line_height(ui, text, self.top_line);
            if self.top_off >= h && self.top_line < last {
                self.top_off -= h;
                self.top_line += 1;
            } else {
                self.top_off = self.top_off.min(h);
                break;
            }
        }
        self.clamp_bottom(ui, text);
        if std::mem::take(&mut self.reveal_caret) {
            self.reveal_caret_wrapped(ui, text);
        }
        self.sync_scroll_y();
    }

    /// Scrolls just far enough that the row the caret is on is inside the pane.
    fn reveal_caret_wrapped(&mut self, ui: &egui::Ui, text: &Buffer) {
        let line = text.line_of_char(self.caret);
        let col = self.caret - text.line_start(line);
        let galley = self.layout_line(ui, text, line, true);
        let (_, row) = Self::row_starts(&galley, text.line_len(line), col);
        let row_y = galley.rows.get(row).map_or(0.0, |r| r.pos.y);
        if line < self.top_line || (line == self.top_line && row_y < self.top_off) {
            // Above the window: bring its row to the top.
            self.scroll_pending = 0.0;
            self.top_line = line;
            self.top_off = row_y;
            return;
        }
        if line - self.top_line > 4_000 {
            // Far below: put the row at the bottom rather than measuring every line
            // on the way there.
            self.scroll_pending = 0.0;
            self.top_line = line;
            self.top_off = row_y;
            self.walk(ui, text, -(self.view_h - self.scroll_h).max(0.0));
            return;
        }
        let mut y = -self.top_off;
        for l in self.top_line..line {
            y += self.line_height(ui, text, l);
        }
        let bottom = y + row_y + self.scroll_h;
        if bottom > self.view_h {
            self.scroll_pending = 0.0;
            self.scroll_by(ui, text, bottom - self.view_h);
        }
    }

    /// Moves the window to a scroll position in points, held within the document,
    /// and stays there: whatever was gliding towards somewhere else is cancelled.
    ///
    /// Every gesture but the wheel goes through this, so none of them can disagree
    /// about where the bottom is, and none of them can be dragged back by a glide
    /// that was still running.
    fn scroll_to(&mut self, y: f32) {
        self.scroll_pending = 0.0;
        self.set_scroll(y);
    }

    /// Sets the scroll position in points, clamped to the document.
    ///
    /// Without wrapping the position is exact. With it, a distance is only an
    /// estimate of where a line is — it is what the scrollbar deals in — so the
    /// window is anchored at the line the estimate falls in, and the frame settles
    /// the exact anchor against the real line heights.
    fn set_scroll(&mut self, y: f32) {
        self.scroll_y = y.clamp(0.0, self.max_scroll_y());
        // The line the window starts at, which is what everything else is written
        // in terms of. Derived rather than stored, because the two can only ever
        // disagree if one of them is written without the other.
        //
        // Clamped to the last line there is a windowful of, because the scroll
        // range is worked out from the pane's height and the row height and the two
        // are measured independently: on a pane a single row tall, a scroll
        // position can name a line the document has no second line for, and the
        // window then shapes nothing at all.
        let h = self.est_h();
        let line = (self.scroll_y / h) as usize;
        self.top_line = if self.wrapping {
            line.min(self.lines.saturating_sub(1))
        } else {
            line.min(self.max_top_line())
        };
        self.top_off = self.scroll_y - self.top_line as f32 * h;
    }

    /// The furthest the window can be scrolled down, in points.
    ///
    /// A screenful of the document is always left showing: the last line of a
    /// document is a real line a reader can scroll to, so a window that scrolled
    /// until it was off the bottom would be a window with no way to see the end of
    /// the file. With wrapping the height of the document is an estimate.
    fn max_scroll_y(&self) -> f32 {
        let content = self.lines as f32 * self.est_h();
        (content - self.view_h).max(0.0)
    }

    /// The scrollbar's strip, or `None` when there is nothing to scroll.
    ///
    /// A document that fits gets no strip, and no reserved width either: taking
    /// eleven points off the text of every short file to draw a bar for a range
    /// of nothing is a tax paid by most files for a feature few of them need.
    fn scrollbar_rect(&self, rect: egui::Rect) -> Option<egui::Rect> {
        if self.scroll_room() == 0 || rect.height() < THUMB_MIN_H * 2.0 {
            return None;
        }
        // No bar at all when the pane is too narrow to hold one beside some text.
        //
        // The width is a quarter of the pane, so a narrow pane gets a narrow bar
        // and a pane of *no* width gets a bar of no width — a rectangle with left
        // and right the same x, which still claims a press from egui and still
        // pages the window when it is clicked. A window dragged down to nothing is
        // a real thing a reader does, and on that frame the text stopped responding
        // to clicks because an invisible bar was sitting on top of it.
        if rect.width() < SCROLLBAR_W * 2.0 {
            return None;
        }
        let w = SCROLLBAR_W.min(rect.width() * 0.25);
        Some(egui::Rect::from_min_max(
            egui::Pos2::new(rect.right() - w, rect.top()),
            egui::Pos2::new(rect.right(), rect.bottom()),
        ))
    }

    /// The thumb's position and size, as a fraction of the way down the track.
    ///
    /// The size is the fraction of the document on screen and the position is
    /// where that fraction starts, so the two stay consistent with each other:
    /// the thumb reaches the bottom of the track exactly when the last line does.
    fn scrollbar_thumb(&self, bar: egui::Rect) -> egui::Rect {
        let room = self.max_scroll_y().max(1.0);
        let frac =
            (self.view_h / (self.lines.max(1) as f32 * self.scroll_h.max(1.0))).clamp(0.02, 1.0);
        let h = (bar.height() * frac).max(THUMB_MIN_H).min(bar.height());
        // The travel is the track minus the thumb, which is what keeps the thumb
        // inside the track at both ends instead of overshooting by its own size.
        // The position is clamped as well, because a thumb that has left its track
        // is worse than one in the wrong place: it is not a position a reader can
        // interpret at all, and the drag maths is measured against the track.
        let travel = bar.height() - h;
        // The window's position as a fraction of how far it can go, in the same
        // units on both sides. Dividing a line by a scroll range measured in points
        // is a quantity with no meaning at all, and it is what left the thumb a
        // third of the way down a track the window had been scrolled to the bottom
        // of.
        let at = (travel * (self.scroll_y / room)).clamp(0.0, travel);
        egui::Rect::from_min_size(
            egui::Pos2::new(bar.left(), bar.top() + at),
            egui::vec2(bar.width(), h),
        )
    }

    /// Takes a drag or a click on the scrollbar, and nothing else.
    ///
    /// Separate from painting it, because the thumb has to be drawn where the
    /// drag has just put it. Doing both in one call at paint time means the bar
    /// trails the pointer by a frame, which is the same lag the caret used to
    /// have and the same reason it was moved.
    ///
    /// Three gestures, because a bar that only drags is not much use: press on
    /// the thumb to drag it, press on the track above or below to page, and
    /// press and hold to keep paging. The top line is the only state involved, so
    /// there is no scroll offset to keep in step with it.
    fn scrollbar_drag(&mut self, ui: &egui::Ui, bar: egui::Rect, pane: egui::Rect) {
        // The bar grows as the pointer nears it and shrinks back when it leaves, and it
        // stays grown for as long as the thumb is being dragged, however far the
        // pointer strays from it. Near means within reach of the bar horizontally and
        // anywhere along the pane, since a pointer coming in from the side is on its way
        // to the bar wherever it is vertically.
        let near = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
            p.x >= bar.left() - BAR_NEAR
                && p.x <= bar.right() + 8.0
                && p.y >= pane.top()
                && p.y <= pane.bottom()
        });
        self.bar_grow = ui.ctx().animate_bool_with_time(
            egui::Id::new("editor-scrollbar-grow"),
            near || self.scrollbar_grabbed,
            0.14,
        );
        let thumb = self.scrollbar_thumb(bar);
        // The area that answers a press is as wide as the bar has grown, reaching left
        // over the edge of the text: a target that is only as wide as the thin thumb
        // would be a target nobody can be expected to hit.
        let reach = BAR_REACH * self.bar_grow;
        let hit = egui::Rect::from_min_max(egui::Pos2::new(bar.left() - reach, bar.top()), bar.max);
        let thumb_hit = egui::Rect::from_min_max(
            egui::Pos2::new(thumb.left() - reach, thumb.top()),
            thumb.max,
        );
        let resp = ui.interact(
            hit,
            egui::Id::new("scrollbar"),
            // Not focusable: a scroll bar that could take the keyboard is a neighbour
            // for an arrow key to move the focus to, away from the text.
            egui::Sense::CLICK | egui::Sense::DRAG,
        );
        // Whether the pointer went down on the thumb, and where inside it,
        // remembered for as long as the button is held. Both have to be remembered
        // rather than looked at each frame: a drag is only recognised once the
        // pointer has *left* the thumb, and by then the thumb has moved with the
        // window on every frame of the drag, so anything measured against the
        // thumb as it is now is measured against a moving target.
        let press = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started() {
            self.scrollbar_grabbed = press.is_some_and(|p| thumb_hit.contains(p));
            self.scrollbar_grab = press
                .map(|p| (p.y - thumb.top()).clamp(0.0, thumb.height()))
                .unwrap_or(thumb.height() * 0.5);
        }
        if !ui.input(|i| i.pointer.any_down()) {
            self.scrollbar_grabbed = false;
        }
        if self.scrollbar_grabbed
            && let Some(pos) = resp
                .interact_pointer_pos()
                .or(ui.input(|i| i.pointer.hover_pos()))
        {
            // Dragged by the point it was grabbed at, so the pointer keeps its
            // place under the cursor rather than the thumb jumping to catch up.
            let travel = (bar.height() - thumb.height()).max(1.0);
            let from = (pos.y - bar.top() - self.scrollbar_grab).clamp(0.0, travel);
            self.scroll_to((from / travel) * self.max_scroll_y());
        } else if resp.clicked_by(egui::PointerButton::Primary)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            // On the track, a page at a time in the direction pressed. A page
            // rather than a jump, because a jump on a 90,000-line file throws away
            // everything in between and there is no way back but the bar.
            let step = if pos.y < thumb.top() {
                -self.view_h
            } else {
                self.view_h
            };
            self.scroll_to(self.scroll_y + step);
        }
    }

    /// Draws the scrollbar at wherever the drag has put it.
    fn scrollbar_paint(&self, ui: &egui::Ui, bar: egui::Rect) {
        let thumb = self.scrollbar_thumb(bar);
        let painter = ui.painter();
        let grow = self.bar_grow;
        // The strip set aside for the bar is painted as it always was, near enough the
        // colour of the page to be a hint and no more.
        painter.rect_filled(bar, egui::CornerRadius::ZERO, crate::theme::c::CODE_BG);
        // Grown, the track widens leftwards over the edge of the text and shows itself,
        // so the target is visible as well as big.
        if grow > 0.01 {
            let width = bar.width() + (THUMB_WIDE + 3.0 - bar.width()) * grow;
            painter.rect_filled(
                egui::Rect::from_min_max(egui::Pos2::new(bar.right() - width, bar.top()), bar.max),
                egui::CornerRadius::ZERO,
                crate::theme::c::CODE_BG.gamma_multiply(0.55 + 0.45 * grow),
            );
        }
        // Thin at rest and thick near the pointer, a step brighter each time it is
        // wanted more: the rest of the interface is greys a few points apart, and a bar
        // that cannot be seen cannot be the thing a reader reaches for.
        let width = THUMB_THIN + (THUMB_WIDE - THUMB_THIN) * grow;
        let right = bar.right() - 2.0;
        let colour = if self.scrollbar_grabbed || ui.rect_contains_pointer(bar) {
            crate::theme::c::TEXT_DIM
        } else if grow > 0.5 {
            crate::theme::c::TEXT_GHOST
        } else {
            crate::theme::c::BORDER
        };
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::Pos2::new(right - width, thumb.top() + 1.0),
                egui::Pos2::new(right, thumb.bottom() - 1.0),
            ),
            egui::CornerRadius::same((width / 2.0).clamp(1.0, 5.0) as u8),
            colour,
        );
    }

    /// Home and End while text wraps: the start or end of the *row* the caret is on,
    /// not of the whole logical line.
    ///
    /// A line five rows tall has its start four rows above and its end four rows
    /// below, which is nowhere a reader pressing Home or End is looking. Every
    /// editor that wraps takes them to the edges of the row on screen. The first row
    /// of a line keeps the smart Home that goes to the code before column zero.
    fn goto_row_edge(&mut self, ui: &egui::Ui, text: &Buffer, start: bool, extend: bool) {
        let line = text.line_of_char(self.caret);
        let col = self.caret - text.line_start(line);
        let galley = self.layout_line(ui, text, line, true);
        let (starts, row) = Self::row_starts(&galley, text.line_len(line), col);
        if start && row == 0 {
            self.goto_line_start(text, extend);
            return;
        }
        let base = text.line_start(line);
        let last_row = row + 2 == starts.len();
        let at = if start {
            base + starts[row]
        } else if last_row {
            base + starts[row + 1]
        } else {
            // In front of the space the line broke on, so the caret is drawn at the
            // end of this row and not at the start of the next.
            base + starts[row + 1].saturating_sub(1).max(starts[row])
        };
        self.place(at, extend);
        self.scroll_to_caret(text);
    }

    /// Moves the caret to the start or the end of its line, and brings that line
    /// on screen.
    fn goto_line_edge(&mut self, text: &Buffer, start: bool, extend: bool) {
        let at = if start {
            self.line_start_char(text)
        } else {
            self.line_end_char(text)
        };
        self.place(at, extend);
        self.scroll_to_caret(text);
    }

    /// Home, which is *smart*: it goes to the first character that is not
    /// whitespace, and pressing it again from there goes to the very start of
    /// the line.
    ///
    /// Column zero as the only answer makes Home useless on an indented line,
    /// where the first press scrolls sideways for no reason and a second press is
    /// needed to reach the code. This is what every editor settled on, and it
    /// still reaches column zero — just on the second press rather than the
    /// first.
    ///
    /// The rule is a comparison rather than a remembered position, so it needs no
    /// state and cannot be left out of step with the caret: further along than
    /// the code means "take me to the code", anywhere else means "take me to the
    /// start". A blank line has no code, so its two answers are the same place
    /// and the key simply does nothing.
    fn goto_line_start(&mut self, text: &Buffer, extend: bool) {
        let start = self.line_start_char(text);
        let line = text.line_of_char(self.caret);
        let indent = text.line_str(line);
        let code = start + indent.chars().take_while(|c| c.is_whitespace()).count();
        // Toggles: anywhere but the first character of the code goes there, and from
        // there it goes to column zero. From column zero it must go back to the code,
        // or Home on an indented line is a key that only ever works once.
        let at = if self.caret != code { code } else { start };
        self.place(at, extend);
        self.scroll_to_caret(text);
    }

    /// Moves the caret by characters, or by words with Ctrl or Alt held.
    fn move_char(&mut self, text: &Buffer, delta: i32, extend: bool, by_word: bool) {
        let n = text.len_chars();
        // With a selection and no Shift, an arrow key ends the selection at the edge
        // it points to instead of moving from the caret: Left goes to where the
        // selection began and Right to where it ended, as in every editor.
        if !extend && !by_word && self.has_selection(n) {
            let (lo, hi) = self.selection(n);
            self.place(if delta < 0 { lo } else { hi }, false);
            self.scroll_to_caret(text);
            return;
        }
        let next = if by_word {
            self.word_step(text, self.caret, delta)
        } else if delta < 0 {
            crate::editing::cluster_start(text, self.caret)
        } else {
            crate::editing::cluster_end(text, self.caret)
        };
        self.place(next.min(n), extend);
        // Left and Right cross line breaks, so they can carry the caret onto a line
        // that is off the edge of the pane.
        self.scroll_to_caret(text);
    }

    /// The next word boundary in one direction, or the current one when there is
    /// none.
    ///
    /// Both directions land on the *start* of a word, which is the property that
    /// makes stepping through a line feel like a sequence of positions rather
    /// than a random walk. The two directions get there differently:
    ///
    /// - Forwards takes two runs: over the word the caret is in, then over the
    ///   gap after it. From anywhere inside a word, that is the start of the
    ///   *following* word.
    /// - Backwards crosses the run the caret is standing at the end of, and then,
    ///   if that run was a gap, the word before it. From the end of a word that
    ///   is one run and lands on that word's start; from inside a gap it is two,
    ///   because the start of a gap is still a gap and the caret has to go on to
    ///   the word beyond it.
    ///
    /// Walked in byte offsets rather than by collecting the document into a
    /// `Vec<char>` and indexing it. Collecting was one allocation of four bytes
    /// per character and a full decode of the file on every press of a key whose
    /// whole job is to be quick — on a 7 MB file, tens of milliseconds of
    /// stutter per press. The walk is bounded by a word and the gap beside it,
    /// so it costs what it looks like, and the answer comes back as a character
    /// index because that is what the caret speaks in.
    fn word_step(&self, text: &Buffer, from: usize, dir: i32) -> usize {
        fn is_word(c: char) -> bool {
            c.is_alphanumeric() || c == '_'
        }
        let mut at = from.min(text.len_chars());
        if dir > 0 {
            // Over the word the caret is in, then over the gap that follows it.
            for word_first in [true, false] {
                while text.char_at(at).is_some_and(|c| is_word(c) == word_first) {
                    at += 1;
                }
            }
        } else {
            // Walked over the whole text rather than the one line, so stepping
            // left off the end of a line carries on into the last word of the
            // line above, as it does everywhere else.
            let here = is_word(
                at.checked_sub(1)
                    .and_then(|i| text.char_at(i))
                    .unwrap_or(' '),
            );
            // A second run, over words, but only when the first was a gap: the
            // start of a gap is still a gap, and a caret standing there has to go
            // on to the word before it. From the end of a *word* the first run
            // has already arrived at a word's first character, and running on
            // would skip a word.
            for run in 0..if here { 1 } else { 2 } {
                let kind = if run == 0 { here } else { true };
                while at > 0 {
                    let c = text.char_at(at - 1).unwrap_or(' ');
                    if is_word(c) != kind {
                        break;
                    }
                    at -= 1;
                }
            }
        }
        at
    }

    /// Inserts without pushing another undo step, for a key that pushed one.
    fn insert_undoed(&mut self, text: &mut Buffer, s: &str, out: &mut Outcome) {
        let (lo, hi) = self.selection(text.len_chars());
        text.replace(lo, hi, s);
        let at = (lo + s.chars().count()).min(text.len_chars());
        self.set_caret(at, text.len_chars());
        out.edited = true;
    }
}

/// Forgets everything about the document that was open.
///
/// Called when the editor is handed a different file. Nothing is carried over
/// on purpose: the caret, the scroll position and the undo history all
/// describe a document that is no longer on screen, and a stale line index
/// would silently mis-lay-out the new one.
impl Editor {
    pub fn reset(&mut self) {
        *self = Editor::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_step_lands_on_the_start_of_a_word_in_both_directions() {
        let text = Buffer::from("alpha beta gamma");
        let mut e = Editor::default();
        e.ensure_index(&text);
        // Forwards: over the word you are in, then over the gap to the next.
        for (from, want) in [(0, 6), (2, 6), (5, 6), (6, 11), (9, 11), (16, 16)] {
            assert_eq!(e.word_step(&text, from, 1), want, "forwards from {from}");
        }
        // Backwards: one run, over whatever the caret is at the end of.
        for (from, want) in [(16, 11), (10, 6), (11, 6), (6, 0), (2, 0), (0, 0)] {
            assert_eq!(e.word_step(&text, from, -1), want, "backwards from {from}");
        }
        // And it steps over a line boundary rather than stopping at the edge,
        // which is what makes it usable for walking a document rather than one
        // line of it.
        let lines = Buffer::from("one two\nthree four");
        e.ensure_index(&lines);
        assert_eq!(e.word_step(&lines, 4, 1), 8, "on to the next line");
        assert_eq!(e.word_step(&lines, 7, -1), 4, "back into the line above");
        // From the first character of a line, the word before it is the last
        // word of the line above — the newline itself is a gap, and the step
        // crosses it on the way.
        assert_eq!(e.word_step(&lines, 8, -1), 4, "and over the newline");
    }

    /// An editor over `text`, with the caret at its end, which is where a reader
    /// opening a file expects to be.
    fn ed(text: &str) -> (Editor, Buffer) {
        let text = Buffer::from(text);
        let mut e = Editor::default();
        e.ensure_index(&text);
        e.caret = text.len_chars();
        e.anchor = e.caret;
        (e, text)
    }

    /// Moves the caret to `at` characters in, dropping any selection. Directly
    /// rather than through `set_caret`, so a test can place the caret anywhere
    /// without the clamping that method does for real callers.
    fn caret_at(e: &mut Editor, at: usize) {
        e.caret = at;
        e.anchor = at;
    }

    // ---- drawn into a real ui ---------------------------------------------
    //
    // The arithmetic in this module is easy to get wrong in a way no unit test on
    // the mapping can see: the caret, the selection and the gutter all answer
    // "which row is this on" from a row height and a row index, while the text
    // itself is placed by the layout engine using its own idea of that height.
    // When the two disagree the file still looks like text, and the caret is
    // somewhere else entirely, a couple of pixels further off on every line.
    // These tests check the drawn rectangles against the row positions the
    // layout engine actually produced.

    /// Characters in line `NN` of the fixture, newline included.
    fn line_len(i: usize) -> usize {
        format!("line {i:02}").chars().count() + 1
    }

    /// The fixture: forty numbered lines, so a mis-placed row has far to drift.
    fn fixture() -> Vec<String> {
        (0..40).map(|i| format!("line {i:02}")).collect()
    }

    /// Tall enough for all forty lines, so the caret's line is always inside the
    /// shaped window. A caret scrolled out of view is not drawn at all, which
    /// would let these tests pass for the wrong reason.
    fn pane() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 700.0))
    }

    fn headless() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        ctx
    }

    /// Runs one frame, for the cases that only care about the editor's own state
    /// afterwards rather than about what reached the screen.
    fn run(ctx: &egui::Context, input: egui::RawInput, mut f: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(input, &mut f);
        acknowledge(&mut out);
    }

    /// Takes the frame's output and lets go of the font-atlas upload in it.
    ///
    /// A real window hands that upload to the graphics device. A headless test
    /// has nowhere to send it, and epaint asserts that an unapplied one is not
    /// simply dropped, so it is released here on purpose.
    fn acknowledge(out: &mut egui::FullOutput) {
        out.textures_delta.clear();
    }

    /// The text galley the editor drew, which is the one with the most rows.
    fn text_galley(out: &egui::FullOutput) -> &egui::epaint::TextShape {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t),
                _ => None,
            })
            .max_by_key(|t| t.galley.rows.len())
            .expect("the editor drew no text at all")
    }

    /// The screen y of every row the editor drew, from the drawn galley.
    fn drawn_row_tops(out: &egui::FullOutput) -> Vec<f32> {
        let g = text_galley(out);
        g.galley.rows.iter().map(|r| g.pos.y + r.pos.y).collect()
    }

    /// Which row of the fixture a window-local character is on.
    ///
    /// Worked out from the fixture's own text rather than from anything the
    /// editor or the layout engine said, so that a mistake in either shows up as
    /// a disagreement instead of the test quietly agreeing with the bug.
    fn fixture_row_of(local: usize) -> usize {
        let mut at = 0usize;
        for i in 0..fixture().len() {
            let n = line_len(i);
            if local < at + n {
                return i;
            }
            at += n;
        }
        fixture().len() - 1
    }

    /// The character counts of the rows as the layout engine reports them.
    ///
    /// Kept only so the test can assert they are *not* the character ranges: a
    /// row holds one glyph per character except the newline that ends it, which
    /// is consumed as the row break. Adding these up therefore puts every row
    /// boundary one character short, and by the twentieth row it is twenty
    /// characters out, which is how the caret, the gutter numbers and the click
    /// hit-test all came to answer about the wrong line.
    #[test]
    fn a_rows_glyph_count_is_not_its_character_range() {
        let out = draw_with_caret_on(0);
        let glyphs: Vec<usize> = text_galley(&out)
            .galley
            .rows
            .iter()
            .map(|r| r.row.glyphs.len())
            .collect();
        assert_eq!(
            glyphs[0],
            "line 00".chars().count(),
            "a row's glyphs are its visible characters, with the newline consumed \
             as the row break rather than kept"
        );
        assert!(
            glyphs[0] < line_len(0),
            "if this ever changes, the editor's row ranges need revisiting"
        );
        // The trap in numbers: summed over twenty rows, the shortfall is twenty
        // characters, which is two and a half lines.
        let short_by: usize = (0..20).map(|i| line_len(i) - glyphs[i]).sum();
        assert_eq!(short_by, 20, "one character lost per newline");
    }

    #[test]
    fn a_wrapped_line_gets_a_number_once_and_a_caret_on_the_right_row() {
        // Soft wrap is the awkward case: one logical line becomes several visual
        // rows, and nothing in a row says which line it belongs to. Here two long
        // lines are wrapped over four rows, so the row a character lands on is
        // not the line it is on.
        let long = "x".repeat(120);
        let text = format!("{long}\n{long}\nshort");
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 300.0));
        let ctx = headless();
        let opts = Options {
            // Wrapping gives up the gutter, as the toggle says.
            line_numbers: false,
            wrap: true,
            ..Options::default()
        };

        // Somewhere past the first row of the first line: still line 0, but a
        // different visual row.
        let mut buf = Buffer::from(text.as_str());
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut ed = Editor::default();
            caret_at(&mut ed, 100);
            ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
            ed.show(ui, rect, &mut buf, &opts);
        });
        acknowledge(&mut out);
        let g = text_galley(&out);
        assert!(
            g.galley.rows.len() >= 5,
            "120 characters should wrap over several rows, got {}",
            g.galley.rows.len()
        );
        let tops = drawn_row_tops(&out);
        // Character 100 is a hundred characters into line 0.
        assert!(
            (0..120).contains(&100),
            "the caret is inside the first long line"
        );
        let caret = drawn_caret(&out);
        let on = tops
            .iter()
            .position(|t| (caret.top() - t).abs() < 1.0)
            .unwrap_or_else(|| panic!("the caret at {:?} is on no row at all", caret.top()));
        assert!(
            on > 0,
            "character 100 should not be on the first row of a wrapped line, it is on row {on}"
        );

        // And the same document with wrapping off puts that character on row 0,
        // which is what makes the difference visible rather than incidental.
        let mut buf2 = Buffer::from(text.as_str());
        let mut out2 = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut ed = Editor::default();
            caret_at(&mut ed, 100);
            ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
            ed.show(
                ui,
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(3000.0, 300.0)),
                &mut buf2,
                &Options::default(),
            );
        });
        acknowledge(&mut out2);
        let tops2 = drawn_row_tops(&out2);
        let caret2 = drawn_caret(&out2);
        assert!(
            (caret2.top() - tops2[0]).abs() < 1.0,
            "unwrapped, character 100 is on the first row: caret at y={:.1}, row 0 at y={:.1}",
            caret2.top(),
            tops2[0]
        );
    }

    #[test]
    fn the_view_follows_the_caret_off_the_bottom_edge() {
        // The case that is easiest to miss: arrowing down past the last visible
        // line has to scroll, and testing the line the caret *came from* instead
        // of the one it went to leaves it walking off the screen.
        let mut e = Editor::default();
        let mut text = Buffer::from(
            (0..200)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        e.ensure_index(&text);
        e.rows_visible = 10;
        e.scroll_h = 15.0;
        e.view_h = 150.0;
        e.top_line = 0;
        caret_at(&mut e, 0);
        let mut out = Outcome::default();
        for _ in 0..12 {
            e.move_line(&text, 1, false, e.rows_visible);
        }
        assert_eq!(e.top_line, 3, "the caret is on line 12, ten rows down");
        assert!(
            e.top_line <= 12 && 12 < e.top_line + e.rows_visible,
            "the caret's line must be on screen: top {}, rows {}, line 12",
            e.top_line,
            e.rows_visible
        );
        let _ = (&mut text, &mut out);

        // And back up off the top.
        for _ in 0..20 {
            e.move_line(&text, -1, false, e.rows_visible);
        }
        assert_eq!(e.caret, 0);
        assert_eq!(e.top_line, 0, "scrolled back to the top");
    }

    #[test]
    fn a_line_offset_is_right_in_both_directions_and_after_an_edit() {
        let mut text = Buffer::from((0..50).map(|i| format!("l{i}\n")).collect::<String>());
        let expected = |text: &Buffer, line: usize| {
            (0..line)
                .map(|before| text.line_len(before) + 1)
                .sum::<usize>()
        };
        for line in 0..50 {
            assert_eq!(text.line_start(line), expected(&text, line), "line {line}");
        }
        for line in (0..50).rev() {
            assert_eq!(
                text.line_start(line),
                expected(&text, line),
                "line {line}, walking down"
            );
        }
        // And after the text has changed underneath it.
        let end = text.len_chars();
        text.insert(end, "more\n");
        assert_eq!(text.line_start(50), expected(&text, 50));
    }

    #[test]
    fn a_selection_in_wrapped_text_stays_on_its_rows() {
        let long = "y".repeat(200);
        let text = format!("{long}\n{long}");
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 400.0));
        let ctx = headless();
        let opts = Options {
            line_numbers: false,
            wrap: true,
            ..Options::default()
        };
        let mut buf = Buffer::from(text.as_str());
        let mut ed = Editor::default();
        // A selection inside the first wrapped line, away from both edges.
        ed.select(100, 150, 200);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
            ed.show(ui, rect, &mut buf, &opts);
        });
        acknowledge(&mut out);
        let tops = drawn_row_tops(&out);
        let bands = drawn_rects(&out, crate::theme::c::SEL);
        assert!(
            !bands.is_empty(),
            "a selection inside a wrapped line was not drawn at all"
        );
        for band in &bands {
            assert!(
                tops.iter().any(|t| (band.top() - t).abs() < 1.0),
                "selection band at y={:.1} is not level with any row",
                band.top()
            );
        }
    }

    /// The drawn rectangle of the caret: a filled box only a couple of points
    /// wide, which nothing else in the editor is.
    fn drawn_caret(out: &egui::FullOutput) -> egui::Rect {
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r)
                    if r.rect.width() <= CARET_W + 0.5 && r.fill == crate::theme::c::ACCENT =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .next_back()
            .expect("no caret was drawn")
    }

    /// Every filled rectangle of one colour, top edge first.
    fn drawn_rects(out: &egui::FullOutput, fill: egui::Color32) -> Vec<egui::Rect> {
        let mut r: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(s) if s.fill == fill => Some(s.rect),
                _ => None,
            })
            .collect();
        r.sort_by(|a, b| a.top().total_cmp(&b.top()));
        r
    }

    /// Runs the editor over the fixture with the caret on one line, and hands
    /// back what was drawn.
    fn draw_with_caret_on(caret_line: usize) -> egui::FullOutput {
        let lines = fixture();
        let mut text = Buffer::from(lines.join("\n"));
        let rect = pane();
        let caret = (0..caret_line).map(line_len).sum::<usize>() + 3;

        let ctx = headless();
        let mut ed = Editor::default();
        caret_at(&mut ed, caret);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            // Asked for inside the frame, which is the only place egui keeps it:
            // the caret is only drawn while the editor holds focus, and a
            // headless run has to say so itself.
            ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
            ed.show(ui, rect, &mut text, &Options::default());
        });
        acknowledge(&mut out);
        out
    }

    #[test]
    fn the_caret_is_drawn_on_the_line_it_belongs_to() {
        // From the first line to the thirty-fourth. If the caret's row came from
        // multiplying a guessed row height instead of from asking the layout
        // engine where the row went, the two are tens of pixels apart down here
        // and the caret sits beside a different line.
        for line in [0usize, 1, 17, 33] {
            let out = draw_with_caret_on(line);
            // The window starts at the top of the file here, so a document
            // character is also a window-local one.
            let caret_char = (0..line).map(line_len).sum::<usize>() + 3;
            let row = fixture_row_of(caret_char);
            assert_eq!(row, line, "the fixture disagrees with itself");
            let tops = drawn_row_tops(&out);
            let caret = drawn_caret(&out);
            assert!(
                (caret.top() - tops[row]).abs() < 1.0,
                "caret on line {line} is at y={:.1} but that row's text is at y={:.1}",
                caret.top(),
                tops[row]
            );
            // And on the right row horizontally, not merely on the right one
            // vertically: the x has to be the start of the column it names.
            let line_start = m_origin_x(&out) + 3.0 * glyph_advance(&out);
            assert!(
                (caret.left() - line_start).abs() < 1.5,
                "caret on line {line} is at x={:.1} but column 3 starts at x={line_start:.1}",
                caret.left()
            );
        }
    }

    /// Left edge of the text, from the drawn galley.
    fn m_origin_x(out: &egui::FullOutput) -> f32 {
        text_galley(out).pos.x
    }

    /// Width of one character, from the drawn galley.
    fn glyph_advance(out: &egui::FullOutput) -> f32 {
        let g = text_galley(out);
        let row = &g.galley.rows[0];
        // Every fixture line is the same length, so the row's width divided by
        // its characters is the advance.
        row.row.size.x / row.row.glyphs.len().max(1) as f32
    }

    #[test]
    fn every_row_is_the_same_height_as_its_neighbour() {
        // A per-row drift of even half a point is what puts the caret on the
        // wrong line further down the file, so the spacing has to be exact.
        let tops = drawn_row_tops(&draw_with_caret_on(0));
        assert!(tops.len() >= 34, "only {} rows were drawn", tops.len());
        let step = tops[1] - tops[0];
        assert!(step > 4.0, "a row is only {step:.1}pt tall");
        for (i, pair) in tops.windows(2).enumerate() {
            let got = pair[1] - pair[0];
            assert!(
                (got - step).abs() < 0.01,
                "row {i} to {} is {got:.2}pt but the first is {step:.2}pt",
                i + 1
            );
        }
    }

    #[test]
    fn a_selection_is_drawn_as_one_band_on_each_line_it_covers() {
        let lines = fixture();
        let mut text = Buffer::from(lines.join("\n"));
        let rect = pane();
        let from = (0..10).map(line_len).sum::<usize>() + 2;
        let to = (0..12).map(line_len).sum::<usize>() + 4;

        let ctx = headless();
        let mut ed = Editor::default();
        ed.select(from, to, text.chars().count());
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ed.show(ui, rect, &mut text, &Options::default());
        });
        acknowledge(&mut out);
        let tops = drawn_row_tops(&out);
        let bands = drawn_rects(&out, crate::theme::c::SEL);
        assert_eq!(
            bands.len(),
            3,
            "a selection from line 10 to line 12 is three boxes, got {}",
            bands.len()
        );
        for (i, band) in bands.iter().enumerate() {
            let want = tops[10 + i];
            assert!(
                (band.top() - want).abs() < 1.0,
                "selection band {i} is at y={:.1} but row {} is at y={want:.1}",
                band.top(),
                10 + i
            );
        }
    }

    #[test]
    fn the_gutter_numbers_are_level_with_their_own_lines() {
        // The gutter draws each number as its own single-row galley, so these
        // are compared against the rows of the text galley beside them.
        let out = draw_with_caret_on(0);
        let tops = drawn_row_tops(&out);
        let numbers: Vec<f32> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) if t.galley.rows.len() == 1 => Some(t.pos.y),
                _ => None,
            })
            .collect();
        assert!(
            numbers.len() >= 34,
            "only {} line numbers were drawn",
            numbers.len()
        );
        let stray: Vec<f32> = numbers
            .iter()
            .copied()
            .filter(|y| !tops.iter().any(|t| (t - y).abs() < 1.0))
            .collect();
        assert!(
            stray.is_empty(),
            "{} gutter numbers are not level with any text row, e.g. {stray:?}",
            stray.len()
        );
    }

    #[test]
    fn a_click_puts_the_caret_on_the_line_that_was_clicked() {
        // The other direction, through the real input path: a pointer press and
        // release at a known point, and the line the caret lands on.
        let lines = fixture();
        let text = lines.join("\n");
        let rect = pane();
        let ctx = headless();
        let mut ed = Editor::default();
        let mut buf = Buffer::from(text.as_str());

        // Find where row 20 is on screen, by laying the file out first.
        let row = 20usize;
        let at = {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                ed.show(ui, rect, &mut buf, &Options::default());
            });
            acknowledge(&mut out);
            let g = text_galley(&out);
            (
                g.pos.x,
                g.pos.y + g.galley.rows[row].pos.y + g.galley.rows[row].row.size.y / 2.0,
            )
        };

        // Now press and release there.
        let pos = egui::pos2(at.0 + 10.0, at.1);
        let press = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        let release = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        run(&ctx, press, |ui| {
            ed.show(ui, rect, &mut buf, &Options::default());
        });
        run(&ctx, release, |ui| {
            ed.show(ui, rect, &mut buf, &Options::default());
        });

        // The caret should now be somewhere on line 20: past the end of line 19
        // and no further than the end of line 20.
        let lo = (0..row).map(line_len).sum::<usize>();
        let hi = (0..=row).map(line_len).sum::<usize>();
        assert!(
            (lo..=hi).contains(&ed.caret),
            "clicking line {row} put the caret at {}, which is outside {lo}..={hi}",
            ed.caret
        );
        assert!(ed.focused(), "clicking the editor should focus it");
    }

    #[test]
    fn typing_goes_to_the_buffer_and_not_to_the_search_box() {
        // Focus is what decides this, and it is the one thing that cannot be
        // checked by looking at the drawing.
        let mut text = Buffer::from("abc");
        let rect = pane();
        let ctx = headless();
        let mut ed = Editor::default();
        caret_at(&mut ed, 3);

        // With focus, the character is inserted.
        let typed = egui::RawInput {
            events: vec![egui::Event::Text("X".into())],
            ..Default::default()
        };
        run(&ctx, typed, |ui| {
            ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
            ed.show(ui, rect, &mut text, &Options::default());
        });
        assert_eq!(text, "abcX", "focused: the keystroke belongs to the file");
        assert!(ed.focused());

        // Without focus, it is swallowed, so it cannot land in the file behind
        // whatever the user is actually typing into.
        let mut other = Buffer::from("abc");
        let ctx2 = headless();
        let mut ed2 = Editor::default();
        let typed_again = egui::RawInput {
            events: vec![egui::Event::Text("X".into())],
            ..Default::default()
        };
        run(&ctx2, typed_again, |ui| {
            ed2.show(ui, rect, &mut other, &Options::default());
        });
        assert_eq!(other, "abc", "unfocused: nothing should change");
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let (mut e, mut text) = ed("hello world");
        e.select(0, 5, 11);
        let mut out = Outcome::default();
        e.insert(&mut text, "bye", &mut out);
        assert_eq!(text, "bye world");
        assert_eq!(e.caret, 3);
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let (mut e, mut text) = ed("abc");
        caret_at(&mut e, 0);
        let mut out = Outcome::default();
        e.delete(&mut text, true, &mut out);
        assert_eq!(text, "abc");
        assert!(!out.edited, "nothing changed, so nothing should be claimed");
    }

    #[test]
    fn delete_joins_lines_across_the_newline() {
        // The caret sits *after* the newline, which is what Backspace has to
        // reach back over.
        let (mut e, mut text) = ed("ab\ncd");
        caret_at(&mut e, 3);
        let mut out = Outcome::default();
        e.delete(&mut text, true, &mut out);
        assert_eq!(text, "abcd", "the newline should go with it");
    }

    #[test]
    #[ignore]
    fn probe_keystroke_cost_on_a_big_file() {
        use std::time::Instant;
        let line = "let value = compute(alpha, beta) + gamma; // a plausible line\n";
        let big: String = line.repeat(7_300_000 / line.len());
        let (mut e, mut text) = ed(&big);
        let mid = text.len_chars() / 2;
        e.set_caret(mid, text.len_chars());
        let mut out = Outcome::default();
        let t = Instant::now();
        e.insert(&mut text, "x", &mut out);
        eprintln!("insert (new step)      {:?}", t.elapsed());
        let t = Instant::now();
        e.insert(&mut text, "y", &mut out);
        eprintln!("insert (in burst)      {:?}", t.elapsed());
        e.last_edit = None;
        let t = Instant::now();
        e.insert(&mut text, "z", &mut out);
        eprintln!("insert + seal          {:?}", t.elapsed());
        let t = Instant::now();
        let _ = e.undo_redo(&mut text, true);
        eprintln!("undo                   {:?}", t.elapsed());
    }

    #[test]
    fn undo_and_redo_agree_with_a_stack_of_whole_snapshots() {
        // The reference is the old design, kept in the test: a stack of whole
        // buffers. Any sequence of edits, pauses, undos and redos has to leave the
        // text exactly where the reference has it.
        use super::harness::Rng;
        for seed in 0..300u64 {
            let mut rng = Rng::new(0x5eed + seed);
            let (mut e, mut text) = ed("héllo
wörld");
            let mut out = Outcome::default();
            let (mut undo, mut redo): (Vec<String>, Vec<String>) = (vec![], vec![]);
            let mut log = Vec::new();
            for _ in 0..40 {
                let n = text.chars().count();
                match rng.below(6) {
                    0 | 1 => {
                        let at = rng.below(n + 1);
                        e.set_caret(at, n);
                        let s = [
                            "a", "é", "
", "xy",
                        ][rng.below(4)];
                        // A burst joins the last step; a fresh step snapshots.
                        let burst = e.last_edit.is_some_and(|t| {
                            std::time::Instant::now().duration_since(t) <= UNDO_COALESCE
                        });
                        if !burst {
                            undo.push(text.to_text());
                            redo.clear();
                        }
                        e.insert(&mut text, s, &mut out);
                        log.push(format!("insert {s:?} at {at} -> {text:?} burst={burst}"));
                    }
                    2 => {
                        let at = rng.below(n + 1);
                        e.set_caret(at, n);
                        let burst = e.last_edit.is_some_and(|t| {
                            std::time::Instant::now().duration_since(t) <= UNDO_COALESCE
                        });
                        let before = text.to_text();
                        e.delete(&mut text, rng.chance(50), &mut out);
                        if before != text.to_text() && !burst {
                            undo.push(before);
                            redo.clear();
                        }
                        log.push(format!("delete at {at} -> {text:?} burst={burst}"));
                    }
                    3 => {
                        if let Some(t) = e.last_edit {
                            e.last_edit = Some(t - std::time::Duration::from_secs(5));
                        }
                        log.push("pause".into());
                    }
                    _ => {
                        let is_undo = rng.chance(60);
                        let left = text.to_text();
                        let moved = e.undo_redo(&mut text, is_undo);
                        // The reference: the buffer being left goes on the other stack.
                        let from = if is_undo { &mut undo } else { &mut redo };
                        let expect = from.pop();
                        match expect {
                            Some(prev) => {
                                let other = if is_undo { &mut redo } else { &mut undo };
                                other.push(left);
                                assert!(moved, "{log:?}");
                                assert_eq!(text, prev, "seed {seed} {log:?} undo={is_undo}");
                            }
                            None => assert!(!moved, "seed {seed} {log:?}"),
                        }
                        log.push(format!("undo={is_undo} -> {text:?}"));
                    }
                }
            }
        }
    }

    #[test]
    fn undo_puts_the_text_and_the_caret_back() {
        let (mut e, mut text) = ed("start");
        caret_at(&mut e, 5);
        let mut out = Outcome::default();
        e.insert(&mut text, "!", &mut out);
        assert_eq!(text, "start!");
        assert!(e.undo_redo(&mut text, true));
        assert_eq!(text, "start");
        assert_eq!(e.caret, 5);
        assert!(e.undo_redo(&mut text, false), "redo should come back");
        assert_eq!(text, "start!");
    }

    #[test]
    fn undo_treats_a_burst_of_typing_as_one_step() {
        let (mut e, mut text) = ed("x");
        caret_at(&mut e, 1);
        let mut out = Outcome::default();
        e.insert(&mut text, "a", &mut out);
        e.insert(&mut text, "b", &mut out);
        e.insert(&mut text, "c", &mut out);
        assert_eq!(text, "xabc");
        // Three keystrokes in quick succession are one thing to undo, which is
        // what makes undo usable while typing rather than after it.
        assert!(e.undo_redo(&mut text, true));
        assert_eq!(text, "x", "the whole burst went at once");
        assert_eq!(e.caret, 1);
        assert!(!e.undo_redo(&mut text, true), "nothing before it to undo");
    }

    #[test]
    fn undo_keeps_separate_pauses_apart() {
        let (mut e, mut text) = ed("x");
        let mut out = Outcome::default();
        caret_at(&mut e, 1);
        e.insert(&mut text, "a", &mut out);
        // Pretend the last keystroke was long enough ago that this one starts a
        // new step, rather than waiting out the real coalescing window.
        e.last_edit = Some(std::time::Instant::now() - UNDO_COALESCE * 2);
        e.insert(&mut text, "b", &mut out);
        assert_eq!(text, "xab");
        assert!(e.undo_redo(&mut text, true));
        assert_eq!(text, "xa", "only the second keystroke went");
        assert!(e.undo_redo(&mut text, true));
        assert_eq!(text, "x");
        assert_eq!(e.caret, 1);
    }

    #[test]
    fn enter_keeps_the_indent_and_steps_in_after_a_bracket() {
        // `INDENT` is four spaces, so a line already indented by four steps in
        // to eight.
        let (mut e, mut text) = ed("    fn x() {");
        caret_at(&mut e, text.chars().count());
        let mut out = Outcome::default();
        e.enter(&mut text, &mut out);
        assert_eq!(
            text, "    fn x() {\n        ",
            "one level in after the brace"
        );
        assert_eq!(e.caret, text.chars().count(), "and the caret follows it");

        let (mut e, mut text) = ed("  let x = 1;");
        caret_at(&mut e, text.chars().count());
        e.enter(&mut text, &mut out);
        assert_eq!(text, "  let x = 1;\n  ", "no extra level without a bracket");

        // Splitting mid-line keeps that line's own indent, not the next one's.
        let (mut e, mut text) = ed("    let a = 1;\nplain");
        caret_at(&mut e, 14); // end of "    let a = 1;"
        e.enter(&mut text, &mut out);
        assert_eq!(text, "    let a = 1;\n    \nplain");
    }

    #[test]
    fn raising_a_line_swaps_it_with_its_neighbour_and_keeps_the_column() {
        let (mut e, mut text) = ed("one\ntwo\nthree");
        caret_at(&mut e, 5); // "two", column 1
        let mut out = Outcome::default();
        e.raise(&mut text, false, &mut out);
        assert_eq!(text, "two\none\nthree");
        assert_eq!(e.caret, 1, "still column 1, now on the first line");
        assert!(out.edited);

        let (mut e, mut text) = ed("one\ntwo\nthree");
        caret_at(&mut e, 6); // "two", column 2
        e.raise(&mut text, true, &mut out);
        assert_eq!(text, "one\nthree\ntwo");
        assert_eq!(e.caret, 12, "still column 2, now on the last line");
    }

    #[test]
    fn raising_works_when_the_lines_are_different_lengths() {
        // The caret hops by the length of the line it lands on, so equal-length
        // lines would hide a mistake in that arithmetic.
        let (mut e, mut text) = ed("abcd\nefghij\nk");
        caret_at(&mut e, 9); // "efghij", column 4
        let mut out = Outcome::default();
        e.raise(&mut text, false, &mut out);
        assert_eq!(text, "efghij\nabcd\nk");
        assert_eq!(e.caret, 4, "same column, on the line that is now first");
    }

    #[test]
    fn raising_the_last_line_up_past_a_shorter_one_keeps_the_text() {
        let (mut e, mut text) = ed("a\nlong line");
        caret_at(&mut e, 8); // "long line", column 6
        let mut out = Outcome::default();
        e.raise(&mut text, false, &mut out);
        assert_eq!(text, "long line\na");
        assert_eq!(e.caret, 6, "column 6 of the same line, now first");
    }

    #[test]
    fn raising_stops_at_the_ends() {
        let (mut e, mut text) = ed("one\ntwo");
        caret_at(&mut e, 0);
        let mut out = Outcome::default();
        e.raise(&mut text, false, &mut out);
        assert_eq!(text, "one\ntwo", "already on the first line");
        assert!(!out.edited);

        caret_at(&mut e, 5);
        e.raise(&mut text, true, &mut out);
        assert_eq!(text, "one\ntwo", "already on the last line");
        assert!(!out.edited);
    }

    #[test]
    fn a_selection_is_sliced_by_character_not_by_byte() {
        // The accented characters are two bytes each, so a byte-based slice
        // would cut one in half.
        assert_eq!(slice_chars(&Buffer::from("héllo wörld"), 0, 5), "héllo");
        assert_eq!(slice_chars(&Buffer::from("héllo"), 1, 3), "él");
        // Out of range clamps instead of panicking.
        assert_eq!(slice_chars(&Buffer::from("hi"), 0, 99), "hi");
        assert_eq!(slice_chars(&Buffer::from("hi"), 99, 99), "");
    }

    #[test]
    fn moving_lines_keeps_the_column_and_clamps_at_the_ends() {
        let (mut e, text) = ed("aaa\nbbb\nccc");
        caret_at(&mut e, 5);
        e.move_line(&text, 1, false, 40);
        assert_eq!(e.caret, 9, "column 1 of the next line");
        e.move_line(&text, 1, false, 40);
        assert_eq!(e.caret, 9, "clamped at the last line");
        e.move_line(&text, -9, false, 40);
        assert_eq!(e.caret, 1, "clamped at the first");
    }

    #[test]
    fn extending_moves_the_caret_and_keeps_the_anchor() {
        let (mut e, text) = ed("abcdef");
        caret_at(&mut e, 2);
        e.place(5, true);
        assert_eq!(e.caret, 5);
        assert_eq!(e.selection(text.chars().count()), (2, 5));
        assert!(e.has_selection(text.chars().count()));
        e.place(1, false);
        assert!(
            !e.has_selection(text.chars().count()),
            "a plain move drops the selection"
        );
    }

    #[test]
    fn a_double_click_selects_the_word_under_the_pointer() {
        let (e, text) = ed("let value = 1");
        assert_eq!(e.word_at(&text, 5), (4, 9), "value");
        assert_eq!(e.word_at(&text, 0), (0, 3), "let");
        // A space selects just itself rather than swallowing a neighbour.
        assert_eq!(e.word_at(&text, 3), (3, 4));
    }

    #[test]
    fn the_caret_never_escapes_the_document() {
        let (mut e, text) = ed("abc");
        assert_eq!(e.line_start_char(&text), 0);
        assert_eq!(e.line_end_char(&text), 3);
    }

    #[test]
    fn the_line_count_follows_a_growing_buffer() {
        let (mut e, mut text) = ed("one\ntwo");
        assert_eq!(text.lines(), 2);
        let mut out = Outcome::default();
        caret_at(&mut e, 7);
        e.insert(&mut text, "\nthree", &mut out);
        e.ensure_index(&text);
        assert_eq!(text.lines(), 3);
        assert_eq!(e.lines, 3);
        assert_eq!(text.line_str(2), "three");
    }
}
