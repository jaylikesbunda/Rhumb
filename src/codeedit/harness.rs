//! Drives the editor through real input, for tests.
//!
//! The bugs this editor had were not in the arithmetic. They were in the parts
//! between the keystroke and the buffer: whether the editor had focus, whether
//! it claimed the key before the app did, whether the click position was
//! translated into the character under the pointer, whether the caret ended up
//! drawn on the row its character was on. None of that is visible from a unit
//! test that calls `insert` directly, and all of it is what a user actually
//! touches.
//!
//! So every test here goes through the same path a keystroke does: an
//! `egui::Event` into a real `Ui` on a real frame, and the assertions are made
//! against what the editor did to the buffer *and* against the rectangles it
//! drew. A test that says "click on line 20, and the caret is on line 20" is
//! worth more than one that says `cursor_from_pos` returns what
//! `cursor_from_pos` returns.
//!
//! Nothing here is timed, so results do not depend on the machine.

use super::{Editor, ID, Options, RowSpan};
use crate::buffer::Buffer;
use egui::{Modifiers, Pos2, Rect, Vec2};

/// A deterministic source of test data, so a failure is always reproducible.
///
/// A hand-rolled generator rather than a crate: the sequences have to be
/// reproducible across machines and runs, and a fixed algorithm is the only way
/// to get that without pinning a dependency's version.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    /// The usual constants from Numerical Recipes: cheap, and good enough to
    /// shake out index arithmetic.
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent as usize
    }
}

/// Words that between them hit every branch of the tokenizer.
const WORDS: &[&str] = &[
    "let",
    "fn",
    "pub",
    "return",
    "if",
    "else",
    "match",
    "struct",
    "impl",
    "use",
    "as",
    "in",
    "value",
    "count",
    "total",
    "result",
    "error",
    "input",
    "index",
    "0x1F",
    "1.5e-9",
    "123_456",
    "\"a string\"",
    "`raw`",
    "// a comment",
    "# not a comment",
    "'",
    "\"",
    "don't",
    "x",
    "()",
    "{}",
    "[]",
    ";",
    "->",
    "=>",
    "é",
    "→",
    "  indented",
    "\ttabbed",
    "",
];

thread_local! {
    /// One egui context for the thread, with the fonts already loaded.
    static SHARED: std::cell::RefCell<Option<egui::Context>> =
        const { std::cell::RefCell::new(None) };
    /// How many harnesses on this thread are alive right now.
    static LIVE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The clock the next harness starts at. Each one starts a long way after the last
    /// finished, so nothing a previous harness did - a click, say - is still recent
    /// enough for egui to count the next one's input as a continuation of it.
    static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(1.0) };
}

/// A context for a new harness.
///
/// Building one is most of what a harness costs - about a millisecond and a half,
/// nearly all of it loading the fonts - and the sweeps make tens of thousands of
/// them. So a thread keeps one and hands it to each harness in turn, wiped: egui's
/// memory (focus, hover, ids) and the display scale are reset, and what carries
/// over is only the parsed fonts and the shaped-text cache, which are pure. A
/// harness made while another is still alive on the thread gets a context of its
/// own, so a test holding two never has one wipe the other.
fn acquire_context() -> egui::Context {
    let live = LIVE.with(|l| {
        let n = l.get();
        l.set(n + 1);
        n
    });
    if live > 0 {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        return ctx;
    }
    let ctx = SHARED.with(|s| {
        s.borrow_mut()
            .get_or_insert_with(|| {
                let ctx = egui::Context::default();
                ctx.set_fonts(crate::theme::fonts());
                ctx
            })
            .clone()
    });
    ctx.memory_mut(|m| *m = Default::default());
    ctx.set_pixels_per_point(1.0);
    ctx
}

impl Drop for Harness {
    fn drop(&mut self) {
        LIVE.with(|l| l.set(l.get().saturating_sub(1)));
    }
}

/// The editor under test, driven one frame at a time.
pub struct Harness {
    ctx: egui::Context,
    ed: Editor,
    text: Buffer,
    rect: Rect,
    opts: Options,
    /// The last frame's shapes, kept so a test can look at what was drawn.
    drawn: egui::FullOutput,
    /// The frames run so far, for tests that care about steady-state cost.
    frames: u32,
    /// Whether the editor claimed the last clipboard shortcut.
    took_clipboard: bool,
    /// Whether every frame is also turned into triangles, as a real window does after
    /// the editor has drawn. Off by default because the correctness tests do not need
    /// it and there are a great many frames; on for the speed tests, so that what they
    /// time includes the part of a frame the editor's own drawing is only the start of.
    tessellate: bool,
    /// The time of the next frame, in seconds. Advances a sixtieth of a second a frame.
    time: f64,
}

impl Harness {
    /// A fresh editor over `text`, focused, in a pane big enough for it.
    pub fn new(text: &str) -> Harness {
        Harness::with_options(text, Options::default(), Vec2::new(600.0, 600.0))
    }

    pub fn with_options(text: &str, opts: Options, size: Vec2) -> Harness {
        let ctx = acquire_context();
        let h = Harness {
            ctx,
            ed: Editor::default(),
            text: Buffer::from(text),
            rect: Rect::from_min_size(Pos2::ZERO, size),
            opts,
            drawn: egui::FullOutput::default(),
            frames: 0,
            took_clipboard: false,
            tessellate: false,
            time: CLOCK.with(|c| {
                let t = c.get();
                c.set(t + 1_000.0);
                t
            }),
        };
        // One frame straight away, so the layout exists before a test asks where
        // anything is. Without it the first `pos_of` has no galley to measure,
        // and the failure would be a confusing "drew no text" rather than an
        // arithmetic one.
        let mut h = h;
        h.frame();
        h
    }

    // ---- driving ----------------------------------------------------------

    /// One frame with no input at all.
    pub fn frame(&mut self) {
        self.send(egui::RawInput::default(), true);
    }

    /// One frame carrying `input`.
    fn send(&mut self, mut input: egui::RawInput, focus: bool) {
        // A timestamp on every frame, as a real window gives, advancing at sixty frames a
        // second. Without one egui sees every click at the same instant, and two clicks
        // in different places on different documents are a double click.
        input.time = Some(self.time);
        self.time += 1.0 / 60.0;
        if self.frames == 0 {
            // egui drops a `TexturesDelta` that nobody applied, and a headless
            // run has nowhere to send the font atlas. A real window hands it to
            // the graphics device; here it is released on purpose.
            input = egui::RawInput {
                events: input.events,
                ..input
            };
        }
        let Self {
            ctx,
            ed,
            text,
            rect,
            opts,
            ..
        } = self;
        // egui tracks a pointer drag over several frames, and a click only
        // registers as one if the release lands in the frame after the press.
        // Both are the platform's behaviour, so the harness has to reproduce
        // them or it stops testing the editor and starts testing the harness.
        let r = *rect;
        let focus = focus && self.frames == 0;
        let mut took = false;
        // The caret blinks by hiding itself for part of each second, and a test
        // that looked for it during the hidden phase would conclude the editor
        // had not drawn one at all. Held in the visible phase here, so "was the
        // caret drawn" means what it says. Nothing else in the editor reads this.
        let mut out = ctx.run_ui(input, |ui| {
            if focus {
                ui.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
            }
            let outcome = ed.show(ui, r, text, opts);
            took = outcome.clipboard;
        });
        out.textures_delta.clear();
        if self.tessellate {
            // What the window does next with the shapes. The result is thrown away;
            // only the time it takes matters.
            let _ = self
                .ctx
                .tessellate(out.shapes.clone(), out.pixels_per_point);
        }
        self.drawn = out;
        self.frames += 1;
        self.took_clipboard = took;
        // The caret blinks by hiding itself for part of every second, and a test
        // that looked for it during the hidden phase would conclude the editor
        // had drawn no caret at all. Put the phase back afterwards, so the next
        // frame starts from a known point rather than from wherever the last one
        // left it.
        ed.blink = 0.0;
    }

    /// A pause long enough that the next edit is not part of the same burst.
    ///
    /// Undo coalesces keystrokes that arrive close together, which is right for
    /// a person typing and untestable without a clock: the alternative is a test
    /// that either depends on how fast the machine is or reaches into the
    /// editor's private timing to force it.
    pub fn pause(&mut self) {
        if let Some(t) = self.ed.last_edit {
            self.ed.last_edit = Some(t - std::time::Duration::from_secs(5));
        }
    }

    /// A key, pressed and released as a real one would be.
    pub fn key(&mut self, key: egui::Key) {
        self.key_mod(key, Modifiers::default());
    }

    /// A key with modifiers held.
    pub fn key_mod(&mut self, key: egui::Key, m: Modifiers) {
        for pressed in [true, false] {
            self.send(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: m,
                    }],
                    ..Default::default()
                },
                false,
            );
        }
    }

    /// Text as typed, which is how a character reaches an editor. Not a key
    /// press: the window layer turns key presses into this, and an editor that
    /// only understood keys would fail here exactly as it would in the wild.
    pub fn type_text(&mut self, s: &str) {
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Text(s.to_owned())],
                ..Default::default()
            },
            false,
        );
    }

    /// Types `s` the way a person at a keyboard does: for each character a frame with
    /// the key going down and the text it produces, and a later frame with the key
    /// coming up. Enter and Tab are keys, not text.
    ///
    /// [`Harness::type_text`] sends the text alone, which is what a paste or an input
    /// method does. A keyboard sends both, and an editor that handles the key *and*
    /// the text - inserting a newline twice, say - only shows it here.
    pub fn type_like_a_person(&mut self, s: &str) {
        for ch in s.chars() {
            let (key, text) = match ch {
                '\n' => (Some(egui::Key::Enter), None),
                '\t' => (Some(egui::Key::Tab), None),
                ' ' => (Some(egui::Key::Space), Some(" ".to_owned())),
                c => (
                    egui::Key::from_name(&c.to_ascii_uppercase().to_string()),
                    Some(c.to_string()),
                ),
            };
            let shift = ch.is_ascii_uppercase();
            let modifiers = Modifiers {
                shift,
                ..Default::default()
            };
            let mut down = Vec::new();
            if let Some(key) = key {
                down.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
            }
            if let Some(t) = text {
                down.push(egui::Event::Text(t));
            }
            self.send(
                egui::RawInput {
                    events: down,
                    ..Default::default()
                },
                false,
            );
            if let Some(key) = key {
                self.send(
                    egui::RawInput {
                        events: vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: false,
                            repeat: false,
                            modifiers,
                        }],
                        ..Default::default()
                    },
                    false,
                );
            }
        }
    }

    /// Holds `key` down for `repeats` auto-repeats, the way a finger left on an
    /// arrow does: one press, a stream of repeats, one release.
    pub fn hold(&mut self, key: egui::Key, repeats: usize) {
        self.hold_mod(key, Modifiers::default(), repeats);
    }

    /// [`Harness::hold`] with modifiers held.
    pub fn hold_mod(&mut self, key: egui::Key, m: Modifiers, repeats: usize) {
        let event = |pressed, repeat| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: m,
        };
        self.send(
            egui::RawInput {
                events: vec![event(true, false)],
                ..Default::default()
            },
            false,
        );
        for _ in 0..repeats {
            self.send(
                egui::RawInput {
                    events: vec![event(true, true)],
                    ..Default::default()
                },
                false,
            );
        }
        self.send(
            egui::RawInput {
                events: vec![event(false, false)],
                ..Default::default()
            },
            false,
        );
    }

    /// A click as a hand makes it: the pointer arrives and rests for a frame, the
    /// button goes down, a few frames pass, and it comes up. Most of the ways a
    /// click can go wrong need the frames in between.
    pub fn click_like_a_person(&mut self, pos: Pos2) {
        self.pointer_moved(pos);
        self.frame();
        self.pointer(pos, true);
        for _ in 0..4 {
            self.frame();
        }
        self.pointer(pos, false);
        self.frame();
    }

    /// Moves the pointer without pressing anything.
    pub fn pointer_moved(&mut self, pos: Pos2) {
        self.send(
            egui::RawInput {
                events: vec![egui::Event::PointerMoved(pos)],
                ..Default::default()
            },
            false,
        );
    }

    /// A clipboard paste, as the window layer delivers one.
    pub fn paste(&mut self, s: &str) {
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Paste(s.to_owned())],
                ..Default::default()
            },
            false,
        );
    }

    /// One step of an input-method composition, as the window layer delivers
    /// it: the candidate text so far, with nothing committed.
    pub fn ime_preedit(&mut self, s: &str) {
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: s.to_owned(),
                    active_range_chars: None,
                })],
                ..Default::default()
            },
            false,
        );
    }

    /// The input method's committed text: the composition is over and the text
    /// belongs in the document.
    pub fn ime_commit(&mut self, s: &str) {
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Ime(egui::ImeEvent::Commit(s.to_owned()))],
                ..Default::default()
            },
            false,
        );
    }

    /// The input method being dismissed, as a window reports it when the
    /// composition is abandoned without committing anything.
    #[expect(deprecated)]
    pub fn ime_disabled(&mut self) {
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
                ..Default::default()
            },
            false,
        );
    }

    /// The input method's composing text, or `None` when nothing is being
    /// composed.
    pub fn preedit(&self) -> Option<&str> {
        self.ed.preedit.as_deref()
    }

    /// A clipboard copy, as the window layer delivers one.
    pub fn send_copy(&mut self) {
        self.took_clipboard = false;
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Copy],
                ..Default::default()
            },
            false,
        );
    }

    /// A clipboard cut, as the window layer delivers one.
    pub fn send_cut(&mut self) {
        self.took_clipboard = false;
        self.send(
            egui::RawInput {
                events: vec![egui::Event::Cut],
                ..Default::default()
            },
            false,
        );
    }

    /// Whether the editor claimed this frame's clipboard shortcut, which it does
    /// by setting the flag on its outcome.
    pub fn took_clipboard(&self) -> bool {
        self.took_clipboard
    }

    /// Press and release the primary button at `pos`.
    pub fn click(&mut self, pos: Pos2) {
        self.pointer(pos, true);
        self.pointer(pos, false);
    }

    /// Click with modifiers, for extending a selection.
    pub fn click_mod(&mut self, pos: Pos2, m: Modifiers) {
        self.pointer_mod(pos, m, true);
        self.pointer_mod(pos, m, false);
        // The key is let go: without this the modifier stays held for every click after.
        if m != Modifiers::default() {
            self.send(
                egui::RawInput {
                    events: vec![egui::Event::ModifiersChanged(Modifiers::default())],
                    ..Default::default()
                },
                false,
            );
        }
    }

    pub fn pointer(&mut self, pos: Pos2, pressed: bool) {
        self.pointer_mod(pos, Modifiers::default(), pressed);
    }

    pub fn pointer_mod(&mut self, pos: Pos2, m: Modifiers, pressed: bool) {
        // The modifier state has to arrive as its own event, because that is
        // where egui takes it from. A pointer event carries the modifiers that
        // were held, but `input.modifiers` - which is what a widget actually
        // reads - is only updated by `ModifiersChanged` and by key presses.
        // Without this a shift-click looks exactly like a plain click, and the
        // test would end up asserting that the editor ignores a modifier it was
        // never told about.
        let mut events = vec![egui::Event::ModifiersChanged(m)];
        if m == Modifiers::default() {
            // Handing over "no modifiers" every frame would be a lie about a
            // key the user is still holding, so it is only sent when a modifier
            // is actually being pressed.
            events.clear();
        }
        events.push(egui::Event::PointerMoved(pos));
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: m,
        });
        self.send(
            egui::RawInput {
                events,
                ..Default::default()
            },
            false,
        );
    }

    /// Press at `from`, move to `to`, release: a drag-select.
    #[allow(dead_code)]
    pub fn drag(&mut self, from: Pos2, to: Pos2) {
        self.pointer(from, true);
        self.press_and_move_to(to);
        self.pointer(to, false);
    }

    /// Press at the last press point and drag the held pointer to `to`, without
    /// releasing - so a test can look at what a frame drew *during* a drag.
    pub fn press_and_move_to(&mut self, to: Pos2) {
        for step in 1..=4 {
            let from = self.ctx.input(|i| i.pointer.press_origin()).unwrap_or(to);
            let t = step as f32 / 4.0;
            let p = egui::pos2(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.send(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(p)],
                    ..Default::default()
                },
                false,
            );
        }
    }

    /// A wheel movement over the pane, in text rows, as a reader describes it.
    ///
    /// Positive means the view moves towards the end of the document, which is what
    /// "scrolling down" means to the person turning the wheel.
    ///
    /// The negation is here rather than in each test, because egui's sign is the
    /// opposite of the reader's: a positive `smooth_scroll_delta.y` is scrolling
    /// *up*, and that is what the platform reports for a wheel rotated away from
    /// the user. Burying the conversion in a test file is how the editor ended up
    /// scrolling the wrong way while every test still passed.
    ///
    /// egui also smooths a wheel gesture across several frames, applying a fraction
    /// of the movement each time, so one frame only scrolls a little of it. Frames
    /// are pumped until the window stops moving, which is what the caller is asking
    /// about, rather than until some internal counter says the gesture is over.
    pub fn scroll(&mut self, rows: f32) {
        self.raw_wheel(-rows);
    }

    /// One wheel event and one frame, with nothing pumped afterwards, and the scroll
    /// position it reached.
    ///
    /// Separate from [`Self::scroll`] because that one waits for the gesture to
    /// finish, and egui's wheel has momentum: a gesture is not delivered as a
    /// distance but as a velocity that is integrated over the frames that follow, so
    /// the distance a gesture eventually covers is not the distance asked for. That
    /// is the right behaviour for a real wheel and useless for asking "did this
    /// movement move the text by this much", which is the question the smoothness
    /// tests are actually asking.
    pub fn wheel_once(&mut self, rows: f32) -> f32 {
        let centre = self.rect.center();
        self.send(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(centre),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(0.0, -rows),
                        phase: egui::TouchPhase::Start,
                        modifiers: Modifiers::default(),
                    },
                    // A `Move` as well, because that is the phase a platform
                    // actually reports movement in and a gesture made of one `Start`
                    // and no `Move` is not a gesture.
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(0.0, -rows),
                        phase: egui::TouchPhase::Move,
                        modifiers: Modifiers::default(),
                    },
                ],
                ..Default::default()
            },
            false,
        );
        self.scroll_y()
    }

    /// The height of one row, in points, measured from the last frame.
    pub fn row_height(&self) -> f32 {
        let tops = self.row_tops();
        if tops.len() < 2 {
            self.advance()
        } else {
            tops[1] - tops[0]
        }
    }

    /// A wheel gesture in egui's own sign: positive is a wheel rotated upwards,
    /// and the window moves towards the top of the document.
    pub fn raw_wheel(&mut self, rows: f32) {
        let centre = self.rect.center();
        let wheel = |delta: f32, phase| egui::Event::MouseWheel {
            // Lines, not points: Line is scaled by egui's own scroll speed, so a
            // delta of 1 is one text row whatever the font size happens to be.
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, delta),
            phase,
            modifiers: Modifiers::default(),
        };
        self.send(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(centre),
                    wheel(rows, egui::TouchPhase::Start),
                    wheel(rows, egui::TouchPhase::Move),
                ],
                ..Default::default()
            },
            false,
        );
        let mut last = self.scroll_y();
        for _ in 0..500 {
            self.send(egui::RawInput::default(), false);
            let now = self.scroll_y();
            if now == last {
                break;
            }
            last = now;
        }
    }

    /// The text buffer, as the user would see it.
    pub fn text(&self) -> String {
        self.text.to_text()
    }

    pub fn caret(&self) -> usize {
        self.ed.caret
    }

    /// Every selection as `(anchor, caret)`, in the order they stand in the text.
    pub fn cursors(&self) -> Vec<(usize, usize)> {
        self.ed.cursors()
    }

    /// Every caret, in the order they stand in the text.
    pub fn carets(&self) -> Vec<usize> {
        self.cursors().into_iter().map(|c| c.1).collect()
    }

    pub fn cursor_count(&self) -> usize {
        self.ed.cursor_count()
    }

    /// Puts a selection at each of `v`, the last the primary.
    pub fn set_selections(&mut self, v: &[(usize, usize)]) {
        let len = self.text.len_chars();
        let v: Vec<(usize, usize)> = v.iter().map(|&(a, c)| (a.min(len), c.min(len))).collect();
        self.ed.set_selections(&v);
        self.frame();
    }

    /// Carets, with nothing selected, at each of `at`.
    pub fn set_carets(&mut self, at: &[usize]) {
        let v: Vec<(usize, usize)> = at.iter().map(|&p| (p, p)).collect();
        self.set_selections(&v);
    }

    pub fn selection(&self) -> (usize, usize) {
        self.ed.selection(self.text.len_chars())
    }

    pub fn focused(&self) -> bool {
        self.ed.focused()
    }

    /// The selected text, or the empty string when nothing is selected.
    pub fn selected(&self) -> String {
        let n = self.text.len_chars();
        let (lo, hi) = self.ed.selection(n);
        self.slice(lo, hi)
    }

    /// The characters between two character indices.
    pub fn slice(&self, lo: usize, hi: usize) -> String {
        self.text.slice(lo, hi).into_owned()
    }

    // ---- what was drawn ----------------------------------------------------

    /// The editor's text galley: the `TextShape` holding the text it laid out.
    ///
    /// Found by matching the text the editor recorded as shaped, not by picking
    /// the biggest text on the frame. The two agree on any document with more than
    /// one line and disagree on an empty one, where the gutter's "1" is a
    /// single-row galley and the document's nothing is also a single-row galley -
    /// and the line number wins a size comparison, so a test asking what the
    /// editor drew was told it drew "1".
    fn galley(&self) -> &egui::epaint::TextShape {
        self.drawn
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t),
                _ => None,
            })
            .find(|t| t.galley.job.text == self.ed.shaped)
            .or_else(|| {
                self.drawn
                    .shapes
                    .iter()
                    .filter_map(|cs| match &cs.shape {
                        egui::epaint::Shape::Text(t) => Some(t),
                        _ => None,
                    })
                    .max_by_key(|t| t.galley.rows.len())
            })
            .expect("the editor drew no text")
    }

    /// The top of each row, in screen coordinates.
    pub fn row_tops(&self) -> Vec<f32> {
        let g = self.galley();
        g.galley.rows.iter().map(|r| g.pos.y + r.pos.y).collect()
    }

    /// Where the text begins horizontally.
    pub fn text_left(&self) -> f32 {
        self.galley().pos.x
    }

    /// Where the text begins horizontally, as the editor worked it out.
    ///
    /// The same number as [`Self::text_left`] except on a document with nothing
    /// in it, which lays out no galley and so has none to measure. Read from the
    /// editor rather than inferred, because "the widest text on the frame" is the
    /// gutter's line number on an empty document.
    pub fn text_left_known(&self) -> f32 {
        self.ed.origin.x
    }

    /// Width of one character.
    pub fn advance(&self) -> f32 {
        let g = self.galley();
        let row = &g.galley.rows[0];
        // From the spacing of the glyphs themselves. Dividing the row's width by its
        // glyph count is wrong for a row that starts partway along a long line, whose
        // width includes the space in front of its first character.
        let glyphs = &row.row.glyphs;
        if glyphs.len() < 2 {
            return row.row.size.x / glyphs.len().max(1) as f32;
        }
        let n = glyphs.len().min(40);
        (glyphs[n - 1].pos.x - glyphs[0].pos.x) / (n - 1) as f32
    }

    /// The screen position of a character, for clicking on it.
    pub fn pos_of(&self, row: usize, col: f32) -> Pos2 {
        Pos2::new(
            self.text_left() + col * self.advance(),
            self.row_tops().get(row).copied().unwrap_or(self.rect.top()) + self.advance() * 0.4,
        )
    }

    /// The caret's drawn rectangle, or `None` when it is not on screen.
    ///
    /// A caret can legitimately be absent for two reasons, neither of them a
    /// bug: the editor does not have the keyboard, and so draws none; or the
    /// caret is outside the window, because the document is longer than the pane.
    pub fn caret_rect(&self) -> Option<Rect> {
        if !self.ed.focused {
            return None;
        }
        self.drawn
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r)
                    if r.rect.width() <= super::CARET_W + 0.5
                        && r.fill == crate::theme::c::ACCENT =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .next_back()
    }

    /// Every caret that is drawn, one rectangle each.
    pub fn caret_rects(&self) -> Vec<Rect> {
        if !self.ed.focused {
            return Vec::new();
        }
        self.drawn
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r)
                    if r.rect.width() <= super::CARET_W + 0.5
                        && r.fill == crate::theme::c::ACCENT =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .collect()
    }

    /// Every filled rectangle of one colour, top edge first.
    #[allow(dead_code)]
    pub fn rects(&self, fill: egui::Color32) -> Vec<Rect> {
        let mut r: Vec<Rect> = self
            .drawn
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

    /// Which row the caret is drawn on, found from the drawing rather than
    /// asked of the editor, or `None` when the caret is not on screen.
    pub fn caret_row(&self) -> Option<usize> {
        let caret = self.caret_rect()?.top();
        self.row_tops().iter().position(|t| (caret - t).abs() < 1.0)
    }

    /// The line numbers the gutter drew, in order.
    ///
    /// Found by *position*: anything drawn to the left of where the text starts
    /// is the line-number column. Not by content, because content cannot
    /// distinguish the two cases that matter - a document one line long is laid
    /// out as a single row, and a document whose only line is the character `1`
    /// is laid out as text identical to the number in the gutter beside it.
    pub fn gutter_numbers(&self) -> Vec<String> {
        let left = self.text_left_known();
        let mut found: Vec<(f32, String)> = self
            .drawn
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t)
                    if t.galley.rows.len() == 1 && t.pos.x < left - 0.5 =>
                {
                    Some((t.pos.y, t.galley.job.text.to_string()))
                }
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, s)| s).collect()
    }

    /// How many rows the layout produced.
    pub fn row_count(&self) -> usize {
        self.galley().galley.rows.len()
    }

    /// The character range each drawn row should cover, worked out from the
    /// document and the laid-out rows rather than asked of the editor.
    ///
    /// This is an oracle, and it is deliberately not the editor's own code: it
    /// reads the buffer and the row glyph counts and applies the rule itself, so
    /// a mistake in the editor's bookkeeping shows up as a disagreement instead
    /// of the test agreeing with the bug. The rule is that a row's glyphs are
    /// its visible characters with the newline that ends the line consumed as
    /// the row break, so the line has to be stepped over by hand.
    /// The character range of each row, as the editor recorded it.
    ///
    /// This is the editor's own bookkeeping, so it is what the tests *check*
    /// rather than a second opinion on it. An earlier version of this tried to
    /// re-derive the ranges from the row glyph counts, which cannot be done: a
    /// row's glyphs are short of its line by the newline and any trailing blanks
    /// the layout dropped, and working that out from the outside needs the whole
    /// wrapped-line-breaking rule. Reimplementing it in a test would only have
    /// given a second copy of the same mistake to compare against.
    pub fn rows(&self) -> &[RowSpan] {
        self.ed.last_rows()
    }

    /// The character range the editor shaped, which is what the rows must cover
    /// between them: the visible lines of the document, each with the newline
    /// that terminates it, counted from the first visible character.
    pub fn shaped_len(&self) -> usize {
        self.job_text().chars().count()
    }

    /// The text the editor laid out, which is the visible window rather than the
    /// whole document.
    ///
    /// Taken from what the editor recorded rather than measured off the frame,
    /// because an empty document lays out no galley and so has none to read: the
    /// only text on the frame is the line number in the gutter, and asking the
    /// frame what the editor drew returns "1" for a document containing nothing.
    pub fn job_text(&self) -> String {
        self.ed.shaped.clone()
    }

    /// Which row holds a character, or `None` when it is in a gap between two
    /// rows - which is where a newline is, since the layout consumes it as the
    /// row break rather than drawing it.
    #[allow(dead_code)]
    pub fn row_of(&self, local: usize) -> Option<usize> {
        self.rows()
            .iter()
            .position(|r| local >= r.chars.0 && local < r.chars.1.max(r.chars.0 + 1))
    }

    /// Which row a character is *drawn* on: the row holding it, or the row after
    /// the gap it is in. This is the rule the editor uses, restated so a test can
    /// predict where the caret should appear without asking the editor.
    pub fn drawn_row_of(&self, local: usize) -> usize {
        let rows = self.rows();
        if let Some(i) = rows
            .iter()
            .position(|r| local >= r.chars.0 && local < r.chars.1.max(r.chars.0 + 1))
        {
            return i;
        }
        // The newline that ends a line: the caret is at the end of that line's row.
        if let Some(i) = rows.iter().position(|r| r.chars.1 == local) {
            return i;
        }
        rows.iter()
            .position(|r| local < r.chars.0)
            .unwrap_or_else(|| rows.len().saturating_sub(1))
    }

    /// The window's first character, as a document character index.
    ///
    /// The rows are numbered from the first visible line, so this is how far into
    /// the document the shaped text starts.
    pub fn window_base(&self) -> usize {
        let first_line = self.rows().first().map_or(0, |r| r.line);
        (0..first_line).map(|l| self.text.line_len(l) + 1).sum()
    }

    /// The line each drawn row belongs to, in order, counting from zero.
    ///
    /// With wrap on, several rows share a line. This is how a test checks that
    /// the editor's own record of that is sane, which is what the gutter numbers
    /// and the caret are both built from.
    pub fn drawn_line_numbers(&self) -> Vec<usize> {
        self.ed.last_rows().iter().map(|r| r.line).collect()
    }

    /// The first line on screen, counting from zero.
    pub fn top_line(&self) -> Option<usize> {
        self.drawn_line_numbers().first().copied()
    }

    /// Where the editor's scrollbar is, and the thumb on it, or `None` when it
    /// drew neither - which is what a document that fits should do.
    ///
    /// Both are read back from the drawn shapes rather than asked of the
    /// editor, so a test is checking what is on screen and not what the editor
    /// believes about it.
    pub fn scrollbar(&self) -> Option<(Rect, Rect)> {
        // The two are told apart by containment rather than by size, because on a
        // long document the thumb is itself taller than the 40 points a "this is
        // a bar, not a thumb" guess would use, and the guess then picks the
        // thumb as the track and finds no thumb inside it.
        let narrow: Vec<Rect> = self
            .leaf_rects()
            .into_iter()
            .filter(|r| r.width() <= 20.0 && r.height() > 8.0)
            .collect();
        // A plain pair of loops rather than iterator plumbing: `find` hands its
        // predicate a double reference, because the items are themselves already
        // borrowed, and spelling out which reference is which takes more lines
        // than the search does.
        for (i, outer) in narrow.iter().enumerate() {
            for (j, inner) in narrow.iter().enumerate() {
                if i != j && outer.contains_rect(*inner) {
                    return Some((*outer, *inner));
                }
            }
        }
        None
    }

    /// The scrollbar strip the editor worked out, whether or not it drew one.
    #[allow(dead_code)]
    pub fn bar(&self) -> Option<Rect> {
        self.ed.scrollbar_rect(self.rect)
    }

    /// How many rows the editor thinks fit.
    #[allow(dead_code)]
    pub fn rows_visible(&self) -> usize {
        self.ed.rows_visible
    }

    /// The caret's line and column, as the status bar shows them.
    pub fn line_column(&self) -> (usize, usize) {
        self.ed.line_and_column(&self.text)
    }

    /// The text of line `n` of the document.
    #[allow(dead_code)]
    pub fn line_text(&self, n: usize) -> String {
        self.text.line_str(n).into_owned()
    }

    /// How far the text is pushed sideways.
    pub fn scroll_x(&self) -> f32 {
        self.ed.scroll_x
    }

    /// The left edge of the line-number column, or `None` when there is no gutter.
    pub fn gutter_left(&self) -> Option<f32> {
        self.gutter_rect().map(|r| r.left())
    }

    /// The line-number column as it was drawn.
    pub fn gutter_rect(&self) -> Option<Rect> {
        self.leaf_rects()
            .into_iter()
            .find(|r| r.left() < 1.0 && r.width() > 5.0 && r.height() > 100.0)
    }

    // ---- folding ----

    /// The start line of every fold the editor found while drawing, in order.
    ///
    /// Folds are looked up a line at a time, so this is the ones on screen. A test
    /// that folds a small document has all of them.
    #[allow(dead_code)]
    pub fn fold_starts(&self) -> Vec<usize> {
        let mut starts: Vec<usize> = self.ed.visible_folds.iter().map(|(s, _)| *s).collect();
        starts.sort_unstable();
        starts.dedup();
        starts
    }

    /// Whether the fold opened by `line` is closed.
    #[allow(dead_code)]
    pub fn fold_closed(&self, line: usize) -> bool {
        self.ed.closed.contains_key(&line)
    }

    /// Where the fold chevron for `line` was drawn last frame, if it has one.
    ///
    /// Read from what the editor recorded while painting rather than guessed at,
    /// so a test clicks where the chevron really is.
    #[allow(dead_code)]
    pub fn fold_chevron(&self, line: usize) -> Option<Pos2> {
        self.ed
            .fold_buttons
            .iter()
            .find(|(l, _)| *l == line)
            .map(|(_, r)| r.center())
    }

    /// Closes or opens the fold at `line` as a click on its chevron would, and
    /// leaves a frame drawn.
    ///
    /// A click is used when the chevron is on screen, so the real pointer path is
    /// exercised; otherwise the same command is run directly, so a test can fold
    /// a line that has scrolled out of view.
    #[allow(dead_code)]
    pub fn toggle_fold(&mut self, line: usize) {
        match self.fold_chevron(line) {
            Some(pos) => self.click(pos),
            None => {
                let text = &self.text;
                self.ed.toggle_fold(line, text);
                self.frame();
            }
        }
    }

    // ---- the find bar ----

    /// Whether the find bar is open.
    pub fn find_open(&self) -> bool {
        self.ed.find.open
    }

    /// What has been typed into the find bar.
    pub fn find_needle(&self) -> &str {
        &self.ed.find.needle
    }

    /// How many matches the bar is holding.
    pub fn find_hits(&self) -> usize {
        self.ed.find.hits.len()
    }

    /// The middle of one of the find bar's buttons, by its tooltip.
    pub fn find_button(&self, tip: &str) -> Pos2 {
        let order = [
            "Close",
            "Next match (Enter)",
            "Previous match (Shift+Enter)",
            "Match whole word only",
            "Match case",
            "Replace (Ctrl+H)",
        ];
        let index = order
            .iter()
            .position(|t| *t == tip)
            .expect("a button the editor knows about");
        self.ed
            .find_buttons
            .get(index)
            .expect("the bar was drawn last frame")
            .center()
    }
    /// How many stretches of text on screen are marked, selected or highlighted.
    ///
    /// Counted as filled rectangles that are not the gutter, the scrollbar or the
    /// find bar - which is to say, "how much of the text has a wash under it".
    pub fn marked_spans(&self) -> usize {
        let text_left = self.text_left();
        let text_top = self.rect().top() + if self.find_open() { 30.0 } else { 0.0 };
        self.leaf_rects()
            .into_iter()
            .filter(|r| {
                r.left() >= text_left - 1.0
                    && r.top() >= text_top - 1.0
                    && r.height() < 40.0
                    && r.width() > 1.0
            })
            .count()
    }

    /// The wash drawn behind selected text, one rectangle per row it covers.
    ///
    /// Read back from the drawn shapes rather than asked of the editor, because
    /// "is the selected text highlighted" is a question about what is on the
    /// screen, and it has been answered wrongly by state that was perfectly
    /// correct.
    pub fn selection_rects(&self) -> Vec<Rect> {
        self.rects(crate::theme::c::SEL)
    }

    /// Whether the editor believes the scrollbar's thumb is being dragged.
    #[allow(dead_code)]
    pub fn grabbed(&self) -> bool {
        self.ed.scrollbar_grabbed
    }

    /// Every piece of text on the frame with the position it was drawn at.
    #[allow(dead_code)]
    pub fn shapes(&self) -> Vec<(Rect, Rect, String)> {
        self.drawn
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => {
                    Some((t.galley.rect, s.clip_rect, t.galley.job.text.to_string()))
                }
                _ => None,
            })
            .collect()
    }

    /// Every filled rectangle on the last frame, flattened out of any nested
    /// vectors.
    fn leaf_rects(&self) -> Vec<Rect> {
        let mut out = Vec::new();
        let mut stack: Vec<&egui::Shape> = self.drawn.shapes.iter().map(|s| &s.shape).collect();
        while let Some(shape) = stack.pop() {
            match shape {
                egui::Shape::Rect(r) => out.push(r.rect),
                egui::Shape::Vec(inner) => stack.extend(inner.iter()),
                _ => {}
            }
        }
        out
    }

    /// The pane's own rectangle.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// The leftmost and rightmost x of every glyph on the last frame.
    #[allow(dead_code)]
    pub fn glyph_extent(&self) -> Option<(f32, f32)> {
        let mut extent: Option<(f32, f32)> = None;
        for (_, rect) in self.text_shapes() {
            extent = Some(match extent {
                None => (rect.left(), rect.right()),
                Some((l, r)) => (l.min(rect.left()), r.max(rect.right())),
            });
        }
        extent
    }

    /// Every piece of text on the last frame, with the rectangle it was clipped
    /// to.
    ///
    /// The clip is the whole of what a test can check about clipping: egui stores
    /// the *unclipped* rectangle and applies the clip when it tessellates, so the
    /// recorded bounds of a long line run far past the edge of the pane however
    /// correctly it is clipped. Asking what it was told to stay inside is both
    /// possible and the actual question.
    pub fn text_shapes(&self) -> Vec<(Rect, Rect)> {
        self.drawn
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.rect, s.clip_rect)),
                _ => None,
            })
            .collect()
    }

    /// Presses and releases the pointer over the scrollbar at `pos`.
    pub fn click_scrollbar(&mut self, pos: Pos2) {
        self.pointer(pos, true);
        self.pointer(pos, false);
    }

    // ---- for the sweeps ----------------------------------------------------

    /// Where the selection is anchored, which is the other end of a drag and the
    /// only part of a selection the caret does not describe.
    pub fn anchor(&self) -> usize {
        self.ed.anchor
    }

    /// The first line on screen, as the editor has it, rather than as the gutter
    /// drew it.
    pub fn top_line_raw(&self) -> usize {
        self.ed.top_line
    }

    /// How far the window is scrolled, in points.
    ///
    /// Read by the wheel tests rather than `top_line`, because a wheel now moves the
    /// text by fractions of a row and the line it starts at does not change until a
    /// whole row has gone by. A test that waited for the line to stop moving would
    /// call the gesture finished while the text was still travelling, and would
    /// report every scroll as ending short of the bottom.
    pub fn scroll_y(&self) -> f32 {
        self.ed.scroll_y
    }

    /// How far the window can scroll down.
    #[allow(dead_code)]
    pub fn scroll_room(&self) -> usize {
        self.ed.scroll_room()
    }

    /// Replaces the pane's size on the next frame, so a test can watch the editor
    /// meet a viewport it has never seen.
    pub fn resize(&mut self, size: Vec2) {
        self.rect = Rect::from_min_size(Pos2::ZERO, size);
    }

    /// Turns soft wrap on or off between frames.
    pub fn set_wrap(&mut self, on: bool) {
        self.opts.wrap = on;
    }

    /// Turns syntax colouring on or off between frames.
    pub fn set_highlight(&mut self, on: bool) {
        self.opts.highlight = on;
    }

    /// Sets the line-comment marker, so the same text can be tokenized as
    /// whatever language the file claims to be.
    #[allow(dead_code)]
    pub fn set_comment(&mut self, comment: &'static str) {
        self.opts.comment = comment;
        self.opts.lang = super::Lang::line_only(comment);
    }

    /// Sets what the language can carry across lines, for colouring.
    #[allow(dead_code)]
    pub fn set_lang(&mut self, lang: super::Lang) {
        self.opts.lang = lang;
    }

    /// Makes the document read-only, which is a state a reader can be looking at
    /// and a keystroke must not be able to leave.
    #[allow(dead_code)]
    pub fn set_editable(&mut self, on: bool) {
        self.opts.editable = on;
    }

    /// Puts the keyboard into the document, as a click would.
    pub fn focus(&mut self) {
        let ctx = &self.ctx;
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(ID)));
        self.send(egui::RawInput::default(), false);
    }

    /// Takes the keyboard away, as clicking the sidebar would.
    pub fn blur(&mut self) {
        let ctx = &self.ctx;
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(ID)));
        self.send(egui::RawInput::default(), false);
    }

    /// Two clicks in a row at the same place, in the frames a platform sends them.
    pub fn double_click(&mut self, pos: Pos2) {
        self.click(pos);
        self.click(pos);
    }

    /// Three clicks at the same place, which is a triple click.
    pub fn triple_click(&mut self, pos: Pos2) {
        self.click(pos);
        self.click(pos);
        self.click(pos);
    }

    /// Opens the find bar and types `needle` into it, as Ctrl+F and then typing
    /// would.
    pub fn find(&mut self, needle: &str) {
        self.key_mod(egui::Key::F, Modifiers::CTRL);
        for ch in needle.chars() {
            self.type_text(&ch.to_string());
        }
    }

    /// Types `with` into the replace field, as Ctrl+H and then typing would.
    pub fn replace_with(&mut self, with: &str) {
        self.key_mod(egui::Key::H, Modifiers::CTRL);
        for ch in with.chars() {
            self.type_text(&ch.to_string());
        }
    }

    /// What the replace field holds.
    pub fn replacement(&self) -> &str {
        &self.ed.find.replacement
    }

    /// Presses one of the replace row's buttons, "Replace" or "All".
    pub fn press_replace(&mut self, all: bool) {
        self.ed.find.pending = Some(all);
        self.frame();
    }

    /// Whether the editor is showing a search bar at all.
    #[allow(dead_code)]
    pub fn find_hits_capped(&self) -> bool {
        self.ed.find.capped
    }

    /// Every shape drawn on the last frame.
    #[allow(dead_code)]
    pub fn drawn_shapes(&self) -> Vec<egui::epaint::ClippedShape> {
        self.drawn.shapes.clone()
    }

    /// An editor that has never had the keyboard: the caret is at the start, nothing
    /// has been clicked, and egui's focus is on something else, which is what a
    /// document looks like the moment it is opened from the file list.
    pub fn never_focused(text: &str) -> Harness {
        let mut h = Harness::new(text);
        h.blur();
        h.blur();
        h
    }

    /// Whether the editor holds the keyboard right now.
    pub fn has_focus(&self) -> bool {
        self.ctx.memory(|m| m.has_focus(egui::Id::new(ID)))
    }

    /// A drag as a hand makes it: the pointer arrives and rests, the button goes
    /// down, the pointer wobbles a little before it commits, travels in many small
    /// steps with a frame each, rests at the end, and the button comes up.
    pub fn drag_like_a_person(&mut self, from: Pos2, to: Pos2) {
        self.pointer_moved(from);
        self.frame();
        self.pointer(from, true);
        self.frame();
        for step in 1..=24 {
            let t = step as f32 / 24.0;
            // A little sideways wobble that dies away, so the path is not a ruler line.
            let wobble = (1.0 - t) * 1.5 * ((step as f32) * 1.7).sin();
            self.pointer_moved(Pos2::new(
                from.x + (to.x - from.x) * t + wobble,
                from.y + (to.y - from.y) * t,
            ));
            self.frame();
        }
        self.pointer_moved(to);
        self.frame();
        self.frame();
        self.pointer(to, false);
        self.frame();
    }

    /// The text of the `row`th drawn row, for a test that talks about what is on
    /// the screen rather than about character indices.
    #[allow(dead_code)]
    pub fn row_text(&self, row: usize) -> String {
        let span = self.rows()[row];
        let base = self.window_base();
        self.text
            .slice(base + span.chars.0, base + span.chars.1)
            .into_owned()
    }

    /// The selection's wash, one rectangle per row it covers, top to bottom.
    pub fn wash_rows(&self) -> Vec<Rect> {
        let mut rects = self.selection_rects();
        rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
        rects
    }

    /// Makes every frame from here on include the tessellation a real window does.
    pub fn set_tessellate(&mut self, on: bool) {
        self.tessellate = on;
    }

    /// What the last frame put on the clipboard, if anything.
    pub fn copied(&self) -> Option<String> {
        self.drawn
            .platform_output
            .commands
            .iter()
            .rev()
            .find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
    }

    /// A wheel turned with Shift held, which scrolls sideways, then frames for it to
    /// settle. Positive `notches` is towards the right, as a person would turn it.
    pub fn wheel_sideways(&mut self, notches: f32) {
        let centre = self.rect.center();
        self.send(
            egui::RawInput {
                events: vec![
                    egui::Event::Key {
                        key: egui::Key::Space,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: Modifiers::SHIFT,
                    },
                    egui::Event::PointerMoved(centre),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(0.0, -notches),
                        phase: egui::TouchPhase::Move,
                        modifiers: Modifiers::SHIFT,
                    },
                ],
                ..Default::default()
            },
            false,
        );
        for _ in 0..3 {
            self.frame();
        }
    }

    /// The scrollbar's thumb as drawn: the smallest narrow tall rectangle on the frame,
    /// which is the thumb and not the track it slides in.
    pub fn drawn_thumb(&self) -> Option<Rect> {
        self.leaf_rects()
            .into_iter()
            .filter(|r| {
                r.width() <= 20.0 && r.height() > 20.0 && r.left() > self.rect.right() - 30.0
            })
            .min_by(|a, b| a.area().total_cmp(&b.area()))
    }

    /// Where the editor put the text's origin last frame, for a probe to print.
    pub fn debug_origin(&self) -> Pos2 {
        self.ed.origin
    }

    /// The scroll state, for a probe to print.
    pub fn scroll_debug(&self) -> String {
        format!(
            "top_line {} top_off {:.1} scroll_y {:.1} pending {:.1} est_h {:.1} lines {}",
            self.ed.top_line,
            self.ed.top_off,
            self.ed.scroll_y,
            self.ed.scroll_pending,
            self.ed.est_h(),
            self.ed.lines
        )
    }

    /// One wheel event of `notches` lines, as a mouse sends it - a `Move` and
    /// nothing else - followed by one frame.
    pub fn wheel_move(&mut self, notches: f32) {
        let centre = self.rect.center();
        self.send(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(centre),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(0.0, notches),
                        phase: egui::TouchPhase::Move,
                        modifiers: Modifiers::default(),
                    },
                ],
                ..Default::default()
            },
            false,
        );
    }

    /// Sets the display scale, as a window moved to a high-density screen would.
    pub fn set_scale(&mut self, pixels_per_point: f32) {
        self.ctx.set_pixels_per_point(pixels_per_point);
        self.frame();
        self.frame();
    }

    /// Whether a search on another thread has not answered yet.
    pub fn find_searching(&self) -> bool {
        self.ed.find.searching
    }

    /// The colour the first occurrence of `word` in the drawn text was given, if the
    /// editor's own text is on screen and has it.
    pub fn colour_of(&self, word: &str) -> Option<egui::Color32> {
        for shape in &self.drawn.shapes {
            let egui::Shape::Text(t) = &shape.shape else {
                continue;
            };
            let job = &t.galley.job;
            if job.text != self.ed.shaped {
                continue;
            }
            let at = job.text.find(word)?;
            return job
                .sections
                .iter()
                .find(|s| s.byte_range.contains(&egui::text::ByteIndex(at)))
                .map(|s| s.format.color);
        }
        None
    }

    /// How long the last frame said it could wait before the next one. `MAX` means
    /// nothing is animating and the window may sleep until there is input.
    pub fn repaint_delay(&self) -> std::time::Duration {
        self.drawn
            .viewport_output
            .values()
            .map(|v| v.repaint_delay)
            .min()
            .unwrap_or(std::time::Duration::MAX)
    }

    /// Runs frames until `done` says so or about ten seconds pass, a few
    /// milliseconds apart so a worker thread gets to run.
    pub fn frames_until(&mut self, mut done: impl FnMut(&Harness) -> bool) -> bool {
        for _ in 0..2000 {
            if done(self) {
                return true;
            }
            self.frame();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        done(self)
    }
}

/// A document of `lines` lines of random words, for the property tests.
pub fn random_doc(rng: &mut Rng, lines: usize) -> String {
    let mut s = String::new();
    for _ in 0..lines {
        let words = 1 + rng.below(12);
        for w in 0..words {
            if w > 0 {
                s.push(' ');
            }
            s.push_str(WORDS[rng.below(WORDS.len())]);
        }
        s.push('\n');
    }
    s
}
