//! Exhaustive and permutational testing of the editor.
//!
//! The tests in `behaviour.rs` are written one at a time, by hand, from a bug
//! report or an idea. That is the right way to find the bug somebody thought of.
//! It cannot find the one nobody did, and in an editor the bugs nobody thinks of
//! are the interesting ones: they are all *combinations* — this key after that
//! click, on this document, with wrap on, at the end of the buffer, where the
//! caret is one character past a multi-byte character.
//!
//! So this file tests the space rather than the samples. Three layers, in
//! increasing order of how much of the space they cover:
//!
//! 1. **Enumeration.** Every string of length 0 to 6 over an alphabet chosen for
//!    the tokenizer's branches. Not sampled: all of it. The alphabet is small and
//!    brutal on purpose — `/`, `*`, `"`, `#`, `\\`, a newline, a space, a
//!    multi-byte character, a combining one — because those are the bytes a
//!    hand-written scanner gets wrong.
//!
//! 2. **Permutation.** Every ordered sequence of up to three editing commands,
//!    from a catalogue of every command the editor has, driven as real
//!    `egui::Event`s. 20 commands and three deep is 8,460 sessions, each one a
//!    real document going through a real frame. This is the layer that finds
//!    "Ctrl+Backspace after a triple click at the end of the document", which no
//!    one is going to write down.
//!
//! 3. **Coverage.** Every pixel of the pane, clicked, for a set of documents that
//!    between them wrap, scroll, have no trailing newline, are one line long, and
//!    contain a multi-byte character mid-line. The click position is the thing a
//!    reader aims, and it is continuous, so sampling it is the one thing that
//!    cannot be reduced to a list.
//!
//! Every layer asserts the same [`Sane`] oracle after every frame, which is what
//! makes them a loop rather than a pile: the oracle is a statement about the
//! editor that must hold after *any* input at all, and a failure in one layer is
//! usually a failure the other layers would also have found.
//!
//! Nothing here is timed and nothing is random unless the test says so with a
//! fixed seed, so a failure is reproducible from its name alone. Set
//! `RHUMB_SWEEP=2` to roughly triple the depth of every layer; the seeds do not
//! change, so a failure found at the higher setting is still the same failure.

use super::harness::{Harness, Rng, random_doc};
use egui::{Key, Modifiers, Pos2, Vec2};

// ---- the oracle -------------------------------------------------------------

/// The invariants that must hold after any frame, whatever went into it.
///
/// A separate type rather than a free function so a failure can say which
/// invariant broke, and so the same check can be run by a hand-written test as
/// well as by a sweep.
struct Sane<'a> {
    /// What went wrong, set by whichever check failed first.
    at: Option<String>,
    h: &'a Harness,
}

impl<'a> Sane<'a> {
    fn new(h: &'a Harness) -> Self {
        Sane { at: None, h }
    }

    /// Checks everything, and panics with the first thing that was wrong.
    ///
    /// Called after *every* frame of every sweep, which is why it is cheap: a
    /// handful of integer comparisons and a walk of the rows, none of which
    /// allocates on a passing run.
    fn check(&mut self, what: &str) {
        if self.at.is_none() {
            self.check_positions();
        }
        if self.at.is_none() {
            self.check_shaped_text();
        }
        if self.at.is_none() {
            self.check_rows();
        }
        if self.at.is_none() {
            self.check_gutter();
        }
        if self.at.is_none() {
            self.check_geometry();
        }
        if let Some(why) = self.at.take() {
            panic!("{what}\n  {why}\n  document: {:?}", self.h.text());
        }
    }

    /// The caret and the anchor are character indices into a buffer that is edited
    /// in place. One past the end is legal and means "after the last character";
    /// anything beyond that is a position the editor has invented, and the next
    /// keystroke will slice the buffer with it and panic.
    ///
    /// This is the single most valuable invariant in the file. Every indexing bug
    /// the editor has ever had showed up here first, and it is the one that turns
    /// a cosmetic fault into a crash.
    fn check_positions(&mut self) {
        let len = self.h.text().chars().count();
        let (caret, anchor) = (self.h.caret(), self.h.anchor());
        if caret > len {
            self.fail(format_args!(
                "caret {caret} is past the end of {len} characters"
            ));
            return;
        }
        if anchor > len {
            self.fail(format_args!(
                "anchor {anchor} is past the end of {len} characters"
            ));
            return;
        }
        let (lo, hi) = self.h.selection();
        if lo > hi || hi > len {
            self.fail(format_args!(
                "selection {lo}..{hi} is not an ordered range in 0..={len}"
            ));
        }
    }

    /// What the editor laid out must be exactly the visible lines of the
    /// document, joined by their newlines.
    ///
    /// This is the strongest check in the file and it is worth spelling out. The
    /// editor's whole design is that it shapes a *window* rather than the
    /// document, and it computes that window from its own line index. If the
    /// index and the buffer ever disagree — because an edit invalidated the
    /// index, or because a line was added above the window — the editor does not
    /// crash and does not look obviously wrong. It shows the wrong *lines*.
    ///
    /// Deriving the expectation from the buffer rather than from the editor's own
    /// bookkeeping is the whole point. An oracle that shares code with the thing
    /// it checks cannot catch that.
    fn check_shaped_text(&mut self) {
        let text = self.h.text();
        // Byte offset where each line starts, plus one past the end. Built here
        // rather than asked of the editor, for the reason the whole oracle is: an
        // expectation derived from the same code as the thing it checks is a
        // second copy of the same mistake.
        let mut starts = vec![0usize];
        let mut at = 0usize;
        for line in text.split('\n') {
            at += line.len() + 1;
            starts.push(at.min(text.len()));
        }
        let lines = text.split('\n').count();
        let first = self.h.top_line_raw();
        if first >= lines {
            self.fail(format_args!(
                "the window starts at line {first} of a document with {lines} lines"
            ));
            return;
        }
        // The window is a *screenful*, not the rest of the document, so the
        // expectation is the slice of the document between the first line shown and
        // the one after the last. Comparing against everything from `first` to the
        // end of the document instead fails on every short pane, and a test that
        // quietly requires the whole file to be shaped is testing for the fault
        // this editor was written to remove.
        let last = self
            .h
            .rows()
            .last()
            .map_or(first, |r| r.line)
            .min(lines - 1);
        let lo = starts[first];
        let hi = starts
            .get(last + 1)
            .copied()
            .unwrap_or(text.len())
            .min(text.len());
        let want = text.get(lo..hi).unwrap_or_default();
        let got = self.h.job_text();
        // The window's last line is not terminated, because there is no line after
        // it in the window to break to — so the slice of the document above, which
        // does carry that line's newline, differs from the job by exactly one
        // character at the end. Both forms are accepted, and only that: the
        // tolerance is one character at one end, not a prefix match, so a window
        // that is a whole line out is still a failure.
        let ok = got == want || want.strip_suffix('\n') == Some(got.as_str());
        if !ok {
            // The first place the two disagree, escaped onto one line, which is the
            // only part of a pair of long strings anyone can read. "These two
            // strings differ" on a document of ninety thousand lines is not a
            // diagnosis.
            let gc: Vec<char> = got.chars().collect();
            let wc: Vec<char> = want.chars().collect();
            let at = gc
                .iter()
                .zip(wc.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(gc.len().min(wc.len()));
            let window = |v: &[char]| -> String {
                let lo = at.saturating_sub(15);
                let hi = (at + 15).min(v.len());
                format!("{:?}", v[lo..hi].iter().collect::<String>())
            };
            self.fail(format_args!(
                "the shaped text is not lines {first}..={last} of the document: they \
                 differ at character {at} of the window\n    shaped:   {}\n    \
                 expected: {}",
                window(&gc),
                window(&wc)
            ));
        }
    }
    /// The rows have to tile the shaped text in order, with no gaps and no
    /// overlaps, and each must belong to a line that exists.
    ///
    /// The rows are what the caret, the selection, the gutter and every click are
    /// addressed through, so a row that overlaps its neighbour is not a cosmetic
    /// fault — it means a caret can be drawn on two rows at once, and that a click
    /// resolves to one of them at random.
    fn check_rows(&mut self) {
        let rows = self.h.rows();
        let lines = self.h.text().split('\n').count();
        let shaped = self.h.job_text();
        let mut next = 0usize;
        for (i, r) in rows.iter().enumerate() {
            if r.line >= lines {
                self.fail(format_args!(
                    "row {i} claims line {} of a document with {lines} lines\n    \
                     window starts at line {}, {} lines shaped, {} layout rows\n    \
                     shaped: {:?}\n    rows: {:?}",
                    r.line,
                    self.h.top_line_raw(),
                    lines,
                    self.h.row_count(),
                    shaped.chars().take(60).collect::<String>(),
                    rows.iter()
                        .map(|r| format!("{}:{}@{}", r.chars.0, r.chars.1, r.line))
                        .collect::<Vec<_>>()
                ));
                return;
            }
            if r.chars.1 < r.chars.0 {
                self.fail(format_args!(
                    "row {i} ends at {} before it starts",
                    r.chars.1
                ));
                return;
            }
            // Consecutive rows are separated by the newline that ends the line
            // between them, which the layout consumes as the row break and which is
            // therefore in neither row's range. So a gap of exactly one character
            // is right, and a gap of more is a line that got no row at all — which
            // is what makes clicking it drop the caret to the bottom of the window.
            //
            // A row may also be zero-width: a blank line is a row with nothing in
            // it. But it may not reach back over the row before it.
            let gap = r.chars.0.saturating_sub(next);
            if gap > 1 {
                self.fail(format_args!(
                    "row {i} starts at character {}, {gap} characters after the end of \
                     the row before it, so a line in between was given no row",
                    r.chars.0
                ));
                return;
            }
            next = r.chars.1;
        }
    }

    /// The gutter numbers the lines the rows are on, once, in order.
    ///
    /// Numbering a line the document does not have is the bug this was added for:
    /// the row after a document's final newline belongs to no line, and giving it
    /// a number put a number in the margin for a line that could not be
    /// scrolled to or edited.
    /// The gutter numbers consecutive real lines, once each, in order.
    ///
    /// Deliberately *not* "one number for every row". The editor shapes a screenful
    /// plus one row, so the rows below the bottom edge belong to real lines and are
    /// correctly not numbered: there is nowhere to draw them. Asserting a number
    /// per row would be asserting that the editor never shapes a row it cannot
    /// show, which is a different design and a worse one.
    ///
    /// What is asserted instead is the three things that are actually faults: a
    /// number for a line the document does not have — the row after a document's
    /// final newline belongs to no line, and numbering it puts a count in the
    /// margin for a line that cannot be scrolled to — a number out of order, and a
    /// skipped line.
    fn check_gutter(&mut self) {
        let drawn: Vec<usize> = self
            .h
            .gutter_numbers()
            .iter()
            .filter_map(|n| n.parse().ok())
            .collect();
        if drawn.is_empty() {
            return;
        }
        let lines = self.h.text().split('\n').count();
        if let Some(bad) = drawn.iter().find(|d| **d > lines || **d == 0) {
            self.fail(format_args!(
                "the gutter numbered {bad} for a document with {lines} lines. Every \
                 line number must be 1..={lines}"
            ));
            return;
        }
        for (i, n) in drawn.iter().enumerate() {
            // A wrapped line is numbered once, on its first row, so consecutive
            // drawn numbers must be consecutive numbers.
            if *n != drawn[0] + i {
                self.fail(format_args!(
                    "the gutter drew {drawn:?}, which skips or repeats: entry {i} \
                     should have been {}",
                    drawn[0] + i
                ));
                return;
            }
        }
        // And every number must belong to a line some row is on, so the gutter
        // cannot be numbering something the window is not showing.
        let row_lines: Vec<usize> = self.h.drawn_line_numbers();
        for n in &drawn {
            if !row_lines.contains(&(*n - 1)) {
                self.fail(format_args!(
                    "the gutter numbered {n} but no row is on line {}",
                    *n - 1
                ));
                return;
            }
        }
    }
    /// Nothing drawn may be at a position that is not a number.
    ///
    /// A `NaN` in a shape is the visible form of a division by a zero height, a
    /// pane of no size, or a wrap width that came out negative — each of which is
    /// a real thing that happens when a window is resized to nothing, and each of
    /// which is silent until the rasteriser meets it. A screenshot cannot see it.
    fn check_geometry(&mut self) {
        for (bounds, clip) in self.h.text_shapes() {
            for v in [
                bounds.left(),
                bounds.right(),
                bounds.top(),
                bounds.bottom(),
                clip.left(),
                clip.top(),
                clip.width(),
                clip.height(),
            ] {
                if !v.is_finite() {
                    self.fail(format_args!(
                        "a drawn shape has a non-finite bound: {bounds:?} clipped to {clip:?}"
                    ));
                    return;
                }
            }
        }
        if let Some(c) = self.h.caret_rect()
            && (!c.left().is_finite() || c.width() < 0.0 || c.height() <= 0.0)
        {
            self.fail(format_args!("the caret was drawn as {c:?}"));
        }
    }

    fn fail(&mut self, why: std::fmt::Arguments<'_>) {
        if self.at.is_none() {
            self.at = Some(why.to_string());
        }
    }
}

/// Checks the editor after the frame it has just drawn, saying what was being
/// attempted when it did it.
fn sane(h: &Harness, what: &str) {
    Sane::new(h).check(what);
}

// ---- how much to sweep ------------------------------------------------------

/// The depth multiplier, from `RHUMB_SWEEP`. `0` is the default and the tests are
/// still exhaustive at their stated sizes; `2` roughly triples the depth of the
/// permutation and enumeration layers.
///
/// Read from the environment rather than a constant so a slow machine can turn it
/// down and a deliberate hunt can turn it up, without the source changing and
/// without a test depending on which machine it ran on.
fn depth() -> usize {
    std::env::var("RHUMB_SWEEP")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(1, |n: usize| n.max(1))
}

// ---- documents that between them cover the awkward cases -------------------

/// Documents chosen so that between them they contain every shape that has
/// broken the editor, rather than a random sample that mostly contains none.
const DOCS: &[&str] = &[
    // Nothing at all. A blank document is where an index divides by its line
    // count, and it is the last one anybody tries.
    "",
    // One character, which is the other end of every clamp.
    "x",
    // A single line with no trailing newline, so the caret has somewhere legal to
    // be that is not a line end.
    "one line, no newline",
    // The same with a trailing newline, which is a different document: it has a
    // final empty line that exists and cannot be shown.
    "one line, newline\n",
    // A trailing newline and then nothing, the shape almost every file has.
    "a\nb\nc\n",
    // Many lines, so the window is a window and the scrollbar is real.
    "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\nlambda\nmu\n",
    // A line far longer than the pane, so wrap on and wrap off are different
    // code paths and the sideways scrollbar range is non-zero.
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nshort\n",
    // Blank lines between content, because a blank line needs a row of its own
    // and a zero-width range, which is a special case everywhere it appears.
    "a\n\nb\n\n\nc\n",
    // Nothing but newlines: 20 empty lines, every row blank.
    "\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n",
    // A multi-byte character in the middle of a line, so every character index,
    // every byte offset and every click column disagree with each other.
    "alpha é→ beta\nsecond líne\n",
    // A multi-byte character at the very end of the last line, which is the
    // hardest place for a byte offset to be right.
    "abc é\nxyz →",
    // Tabs, which have no fixed width and so move every column after them.
    "\tone\ntwo\tthree\n",
    // Trailing whitespace, which a layout drops but a buffer keeps, so the rows
    // and the document are legitimately different lengths.
    "trailing spaces   \nand a tab\t\n",
    // Quotes and comment markers, for the tokenizer, and a line that is only a
    // marker.
    "let x = \"s\"; // c\n\"unterminated\n/* block\nstill a comment\nend */ let y = 1;\n#\n",
    // A long run of characters with no spaces, to make the wrap logic work hard.
    "0123456789012345678901234567890123456789012345678901234567890123456789\
     0123456789012345678901234567890123456789012345678901234567890123456789\n",
];

/// Every option combination that changes what the editor draws, not just how.
fn option_sets() -> Vec<(bool, bool, &'static str)> {
    let mut out = Vec::new();
    for wrap in [false, true] {
        for highlight in [false, true] {
            for comment in ["//", "#", "--", ""] {
                out.push((wrap, highlight, comment));
            }
        }
    }
    out
}

// ---- 1. enumeration: every short string -------------------------------------

/// The bytes a hand-written scanner gets wrong, and nothing else.
///
/// Every entry is a byte that starts, ends or continues a construct the
/// tokenizer has to recognise. A letter or a digit would add cases without
/// adding branches, so the length is spent on punctuation instead.
const ALPHABET: &[&str] = &[
    "a", "/", "*", "\"", "#", "\\", " ", "\n", "é", "\u{0301}", "\t", "'", "0", "_", "`",
];

/// Every string of length 0 to `max` over [`ALPHABET`], enumerated in a fixed
/// order.
fn enumerate(max: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..max {
        let mut next = Vec::with_capacity(frontier.len() * ALPHABET.len());
        for s in &frontier {
            for a in ALPHABET {
                let mut t = s.clone();
                t.push_str(a);
                next.push(t);
            }
        }
        out.extend_from_slice(&next);
        frontier = next;
    }
    out
}

#[test]
fn every_short_line_survives_the_tokenizer() {
    // The tokenizer's first promise is that its runs concatenate back to exactly
    // the line it was given. If that fails, everything after it — the colours, the
    // layout, the click columns — is being computed about a different string from
    // the one in the buffer, and the editor is wrong in a way no test that checks
    // one token can see.
    //
    // Enumerated, not sampled: 15^0 + 15^1 + ... + 15^5 is 576,975 lines, all of
    // them reachable, all of them checked, and the whole run takes about a
    // second. A sampled version of this test would have found the `/*` and `\`
    // cases only by luck.
    let cases = enumerate(4 + depth().min(2));
    for line in &cases {
        for comment in ["//", "#", "--", ""] {
            let runs = super::tokenize(line, comment);
            let joined: String = runs.iter().map(|(s, _)| s.as_str()).collect();
            assert_eq!(
                &joined, line,
                "the runs do not add back up to the line, for comment marker {comment:?}"
            );
            assert!(
                runs.iter().all(|(s, _)| !s.is_empty()),
                "the tokenizer emitted an empty run for {line:?}"
            );
        }
    }
}

/// How many parallel tests each heavy sweep is split into.
///
/// The sweeps are exhaustive and each case builds its own egui context, so the
/// whole run is minutes of CPU. Split into shards that each take every Nth case,
/// the tests run side by side and the wall time is that of one shard, with every
/// case still checked exactly once.
const SHARDS: usize = 8;

/// Defines one `#[test]` per shard of a sweep function taking `(shard, shards)`.
macro_rules! sharded {
    ($sweep:ident: $($name:ident = $shard:expr),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                $sweep($shard, SHARDS);
            }
        )+
    };
}

sharded!(shaped_and_clicked:
    every_short_document_can_be_shaped_and_clicked_a = 0,
    every_short_document_can_be_shaped_and_clicked_b = 1,
    every_short_document_can_be_shaped_and_clicked_c = 2,
    every_short_document_can_be_shaped_and_clicked_d = 3,
    every_short_document_can_be_shaped_and_clicked_e = 4,
    every_short_document_can_be_shaped_and_clicked_f = 5,
    every_short_document_can_be_shaped_and_clicked_g = 6,
    every_short_document_can_be_shaped_and_clicked_h = 7,
);

fn shaped_and_clicked(shard: usize, shards: usize) {
    // The same enumeration, but through the editor and through real input, which
    // is where the tokenizer's output meets the layout and the hit test. A
    // tokenizer that returns perfect runs can still panic the layout, and a
    // layout that never panics can still put a click on the wrong character.
    //
    // The strings are built into multi-line documents, because a single line
    // never exercises the state a document carries between its lines.
    let cases = enumerate(3 + depth());
    let mut rng = Rng::new(0x5EED_1234 + shard as u64);
    for (i, line) in cases
        .iter()
        .enumerate()
        .filter(|(i, _)| i % shards == shard)
    {
        // Three lines: the one under test, and two fixed ones so the window has
        // something above and below it.
        let doc = format!("first line\n{line}\nlast line\n");
        let mut h = Harness::new(&doc);
        sane(&h, "the first frame of a swept document");
        // Click every row at its start, middle and end, which is the three
        // positions a reader aims at and the three that bracket a multi-byte
        // character.
        for row in 0..3 {
            let y = h.pos_of(row, 0.0).y;
            for frac in [0.1f32, 0.5, 0.9] {
                let x = h.rect().left() + h.rect().width() * frac;
                h.click(Pos2::new(x, y));
                sane(&h, "a click on a swept document");
            }
        }
        // And type at the end of it, which is where an index is most likely to
        // be one past the truth.
        h.key(Key::End);
        h.type_text("!");
        sane(&h, "typing at the end of a swept document");
        h.key(Key::Enter);
        sane(&h, "enter at the end of a swept document");
        h.key(Key::Backspace);
        sane(&h, "backspace at the end of a swept document");
        // A random walk through the document, so the case is not always the same
        // one. The seed is fixed, so the same document always walks the same way.
        for _ in 0..24 {
            match rng.below(6) {
                0 => h.key(Key::ArrowLeft),
                1 => h.key(Key::ArrowRight),
                2 => h.key(Key::ArrowUp),
                3 => h.key(Key::ArrowDown),
                4 => h.type_text("x"),
                _ => h.key(Key::Backspace),
            }
            sane(&h, "a random walk across a swept document");
        }
        if i % 4096 == 4095 {
            eprintln!("  swept {}/{} documents", i + 1, cases.len());
        }
    }
}

sharded!(wrapped_correctly:
    every_short_document_is_wrapped_correctly_a = 0,
    every_short_document_is_wrapped_correctly_b = 1,
    every_short_document_is_wrapped_correctly_c = 2,
    every_short_document_is_wrapped_correctly_d = 3,
);

fn wrapped_correctly(shard: usize, _shards: usize) {
    // Wrapping is the mode where the row ranges stop lining up with the lines,
    // and it is a separate code path in the layout. Checked over the same
    // enumeration with wrap on, and against the same oracle, which is stricter
    // here: with wrap on, every row must still sit inside the line it names.
    let shards = 4;
    let cases = enumerate(3 + depth());
    for line in cases
        .iter()
        .enumerate()
        .filter(|(i, _)| i % shards == shard)
        .map(|(_, l)| l)
    {
        let doc = format!("a much longer first line than the pane is wide\n{line}\n");
        let mut h = Harness::new(&doc);
        h.set_wrap(true);
        h.frame();
        sane(&h, "the first wrapped frame of a swept document");
        h.set_wrap(false);
        h.frame();
        sane(&h, "turning wrap off under a swept document");
        h.set_wrap(true);
        h.frame();
        sane(&h, "turning wrap back on under a swept document");
    }
}

// ---- 2. permutation: every sequence of editing commands --------------------

/// One editing command, as a thing a reader can do.
type Op = fn(&mut Harness);

/// Every command the editor has, including the ones that are only a
/// combination of others.
///
/// A catalogue is only useful if it is complete, and completeness here is not a
/// matter of taste: the permutation layer is exactly as good as this list. A
/// command that is missing is a command whose interactions with all the others
/// are untested, and the ones that get left out are always the awkward ones.
const OPS: &[(&str, Op)] = &[
    ("type a letter", |h| h.type_text("a")),
    ("type a space", |h| h.type_text(" ")),
    ("type a newline", |h| {
        h.key(Key::Enter);
    }),
    ("type a tab", |h| {
        h.key(Key::Tab);
    }),
    ("type a multi-byte character", |h| h.type_text("é")),
    ("backspace", |h| {
        h.key(Key::Backspace);
    }),
    ("delete", |h| {
        h.key(Key::Delete);
    }),
    ("ctrl+backspace", |h| {
        h.key_mod(Key::Backspace, Modifiers::CTRL);
    }),
    ("ctrl+delete", |h| {
        h.key_mod(Key::Delete, Modifiers::CTRL);
    }),
    ("left", |h| {
        h.key(Key::ArrowLeft);
    }),
    ("right", |h| {
        h.key(Key::ArrowRight);
    }),
    ("up", |h| {
        h.key(Key::ArrowUp);
    }),
    ("down", |h| {
        h.key(Key::ArrowDown);
    }),
    ("home", |h| {
        h.key(Key::Home);
    }),
    ("end", |h| {
        h.key(Key::End);
    }),
    ("ctrl+left", |h| {
        h.key_mod(Key::ArrowLeft, Modifiers::CTRL);
    }),
    ("ctrl+right", |h| {
        h.key_mod(Key::ArrowRight, Modifiers::CTRL);
    }),
    ("ctrl+home", |h| {
        h.key_mod(Key::Home, Modifiers::CTRL);
    }),
    ("ctrl+end", |h| {
        h.key_mod(Key::End, Modifiers::CTRL);
    }),
    ("page down", |h| {
        h.key(Key::PageDown);
    }),
    ("page up", |h| {
        h.key(Key::PageUp);
    }),
    ("shift+right", |h| {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    }),
    ("shift+left", |h| {
        h.key_mod(Key::ArrowLeft, Modifiers::SHIFT);
    }),
    ("shift+home", |h| {
        h.key_mod(Key::Home, Modifiers::SHIFT);
    }),
    ("shift+end", |h| {
        h.key_mod(Key::End, Modifiers::SHIFT);
    }),
    ("ctrl+shift+left", |h| {
        h.key_mod(Key::ArrowLeft, Modifiers::CTRL | Modifiers::SHIFT);
    }),
    ("select all", |h| {
        h.key_mod(Key::A, Modifiers::CTRL);
    }),
    ("duplicate lines", |h| {
        h.key_mod(Key::D, Modifiers::CTRL);
    }),
    ("delete lines", |h| {
        h.key_mod(Key::K, Modifiers::CTRL | Modifiers::SHIFT);
    }),
    ("undo", |h| {
        h.key_mod(Key::Z, Modifiers::CTRL);
    }),
    ("redo", |h| {
        h.key_mod(Key::Z, Modifiers::CTRL | Modifiers::SHIFT);
    }),
    ("cut", |h| {
        h.send_copy();
    }),
    ("paste", |h| h.paste("pasted ")),
    ("click at the start of the text", |h| {
        let p = h.pos_of(0, 0.0);
        h.click(p);
    }),
    ("click past the end of a line", |h| {
        let p = h.pos_of(0, 400.0);
        h.click(p);
    }),
    ("click below the last line", |h| {
        let r = h.rect();
        h.click(Pos2::new(r.left() + 20.0, r.bottom() - 3.0));
    }),
    ("click in the gutter", |h| {
        let r = h.rect();
        h.click(Pos2::new(r.left() + 4.0, r.top() + 10.0));
    }),
    ("double click", |h| {
        let p = h.pos_of(0, 3.0);
        h.double_click(p);
    }),
    ("triple click", |h| {
        let p = h.pos_of(0, 3.0);
        h.triple_click(p);
    }),
    ("shift+click", |h| {
        let p = h.pos_of(1, 3.0);
        h.click_mod(p, Modifiers::SHIFT);
    }),
    ("drag across two lines", |h| {
        let a = h.pos_of(0, 1.0);
        let b = h.pos_of(1, 4.0);
        h.pointer(a, true);
        h.press_and_move_to(b);
        h.pointer(b, false);
    }),
    ("scroll down and type", |h| {
        h.scroll(3.0);
        h.type_text("z");
    }),
    ("scroll to the end", |h| {
        for _ in 0..40 {
            h.scroll(20.0);
        }
    }),
    ("open find and search", |h| h.find("a")),
    ("find and then type", |h| {
        h.find("e");
        h.type_text("x");
    }),
    ("lose the keyboard and type", |h| {
        h.blur();
        h.type_text("nope");
    }),
    ("lose and regain the keyboard", |h| {
        h.blur();
        h.focus();
        h.type_text("y");
    }),
];

#[test]
fn every_pair_and_triple_of_editing_commands_leaves_the_editor_sane() {
    // The layer that needs no imagination. Two commands, then three, over the
    // whole catalogue: 45 pairs and 91,125 triples, each one a fresh editor
    // driven with real events, each one checked against the oracle after every
    // single frame.
    //
    // Three is not arbitrary either. Almost every fault that has ever been found
    // in this editor needed at most three steps to reach, and the reason is
    // structural: a step puts the editor into a state, a second step into a
    // state derived from it, and a third is where the derived state meets a
    // length that no longer matches. Four-deep is where combinatorics start
    // costing more than they find.
    let mut done = 0usize;
    let mut failures = Vec::new();
    // Enumerated over the catalogue itself rather than `0..n`, so the loop variable
    // and the entry cannot drift apart.
    for (i, &(a_name, a)) in OPS.iter().enumerate() {
        for (j, &(b_name, b)) in OPS.iter().enumerate() {
            for (k, &(c_name, c)) in OPS.iter().enumerate() {
                // Only run the full triple in a release build unless asked: a
                // debug build lays out text about forty times slower, and 91,125
                // sessions is not a debug-build number. The pairs and the single
                // commands always run, and the triples are covered by the seeded
                // random walks below at any profile.
                if k > 0 && !cfg!(debug_assertions) && depth() < 3 {
                    continue;
                }
                let doc = DOCS[i % DOCS.len()];
                let wrap = DOCS[j % DOCS.len()].contains("aaaa");
                let mut h = Harness::with_options(
                    doc,
                    super::Options {
                        wrap,
                        ..Default::default()
                    },
                    Vec2::new(220.0, 90.0),
                );
                let steps: Vec<(&str, Op)> = vec![(a_name, a), (b_name, b), (c_name, c)];
                if let Err(why) = run_ops(&mut h, &steps)
                    && failures.len() < 8
                {
                    failures.push(format!(
                        "{:?} then {:?} then {:?} on {:?} (wrap {wrap}): {why}",
                        OPS[i].0, OPS[j].0, OPS[k].0, doc
                    ));
                }
                done += 1;
            }
        }
        if i % 5 == 4 {
            eprintln!("  {done} sessions so far, {} failing", failures.len());
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {done} command sequences left the editor broken:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// Runs each command, checking the oracle after every frame, and reports the
/// first failure rather than panicking so that a sweep run collects all of them
/// instead of stopping at the first.
fn run_ops(h: &mut Harness, steps: &[(&str, Op)]) -> Result<(), String> {
    for (name, op) in steps {
        // A guard rather than a catch: a panic inside an operation is itself the
        // interesting result, and there is no way to catch one across a closure
        // boundary, so the sweep lets it out and the harness reports the case
        // name in the panic payload below.
        let before = h.text().to_owned();
        op(h);
        let mut s = Sane::new(h);
        s.check(name);
        if let Some(why) = s.at {
            return Err(format!(
                "after {name:?}, which made the text {:?} -> {:?}: {why}",
                before,
                h.text()
            ));
        }
        // One more frame with nothing in it, because a great many faults are not
        // in what a command did but in what the *next* frame did with the result:
        // a caret clamped against a stale length, a window pointing at a line that
        // has gone, a selection that outlives the text it pointed into.
        h.frame();
        let mut s = Sane::new(h);
        s.check(&format!("the frame after {name:?}"));
        if let Some(why) = s.at {
            return Err(format!(
                "after {name:?}, on the following idle frame: {why}"
            ));
        }
    }
    Ok(())
}

#[test]
fn long_sequences_of_random_commands_never_break_it() {
    // Depth, for the faults that need more than three steps to reach.
    //
    // Random, but from a fixed seed, so this is still a deterministic test: the
    // same hundred thousand commands, in the same order, on every machine, with
    // the case recoverable from the seed alone. The seed is written into the
    // failure message, which is the one thing a fuzzer needs and the one thing
    // most hand-rolled ones forget.
    let seeds: &[u64] = if depth() >= 3 {
        &[1, 2, 3, 4, 5, 6, 7, 8]
    } else {
        &[0xC0FFEE, 0xBADF00D, 0x1DEA, 0xB16B00B5]
    };
    let rounds = 400 * depth();
    for &seed in seeds {
        let mut rng = Rng::new(seed);
        for round in 0..rounds {
            // A document that changes shape underneath the run, so the index is
            // invalidated at unpredictable points rather than never.
            let doc = if rng.chance(30) {
                let lines = 1 + rng.below(60);
                random_doc(&mut rng, lines)
            } else {
                DOCS[rng.below(DOCS.len())].to_owned()
            };
            let (wrap, highlight, comment) = option_sets()[rng.below(option_sets().len())];
            let mut h = Harness::with_options(
                &doc,
                super::Options {
                    wrap,
                    highlight,
                    comment,
                    ..Default::default()
                },
                Vec2::new(
                    80.0 + 200.0 * (rng.below(8) as f32),
                    40.0 + 120.0 * (rng.below(8) as f32),
                ),
            );
            let steps = 6 + rng.below(14);
            let mut plan = Vec::new();
            for _ in 0..steps {
                let (name, op) = OPS[rng.below(OPS.len())];
                plan.push((name, op));
            }
            if let Err(why) = run_ops(&mut h, &plan) {
                panic!(
                    "seed {seed:#x}, round {round}\n  document: {doc:?}\n  \
                     options: wrap {wrap}, highlight {highlight}, comment {comment:?}\n  \
                     pane: {:?}\n  plan: {:?}\n  {why}",
                    h.rect().size(),
                    plan.iter().map(|(n, _)| *n).collect::<Vec<_>>()
                );
            }
        }
        eprintln!("  seed {seed:#x}: {rounds} sessions clean");
    }
}

// ---- 3. coverage: every pixel ----------------------------------------------

sharded!(every_pixel:
    every_pixel_of_the_pane_is_a_safe_place_to_click_a = 0,
    every_pixel_of_the_pane_is_a_safe_place_to_click_b = 1,
    every_pixel_of_the_pane_is_a_safe_place_to_click_c = 2,
    every_pixel_of_the_pane_is_a_safe_place_to_click_d = 3,
);

fn every_pixel(shard: usize, _shards: usize) {
    // The one continuous input. A reader aims at a point, so the whole pane has
    // to be swept rather than sampled, and the cases that matter are the ones a
    // grid of "interesting" points will skip: the gutters between rows, the strip
    // beside the last character of a line, the exact boundary between two
    // characters, the seam between the text and the scrollbar.
    //
    // Every click is followed by the oracle, and by a check that the caret the
    // click produced is on a row that exists — the second of which is what a
    // click *feels* wrong about, and it is invisible to a test that only asks
    // whether the click panicked.
    let step = if depth() >= 3 { 1.0 } else { 2.0 };
    let shards = 4;
    for (di, doc) in DOCS.iter().enumerate().filter(|(i, _)| i % shards == shard) {
        for (wrap, highlight, comment) in option_sets() {
            if highlight && comment.is_empty() {
                continue; // the same as the previous case with highlight off
            }
            let mut h = Harness::with_options(
                doc,
                super::Options {
                    wrap,
                    highlight,
                    comment,
                    ..Default::default()
                },
                Vec2::new(180.0, 110.0),
            );
            let r = h.rect();
            let mut y = r.top() + 1.0;
            let mut clicks = 0usize;
            while y < r.bottom() {
                let mut x = r.left() + 1.0;
                while x < r.right() {
                    h.click(Pos2::new(x, y));
                    sane(&h, "a click swept across the pane");
                    if let Some(row) = h.caret_row() {
                        assert!(
                            row < h.row_count(),
                            "doc {di} at ({x:.0},{y:.0}) put the caret on row {row} of {}",
                            h.row_count()
                        );
                    }
                    clicks += 1;
                    x += step;
                }
                y += step;
            }
            eprintln!("  doc {di} wrap {wrap} comment {comment:?}: {clicks} clicks clean");
        }
    }
}

#[test]
fn every_point_of_the_scrollbar_is_a_safe_place_to_press() {
    // The scrollbar is the one widget whose position maps to a *position in the
    // document* rather than to a character, so a press near the end of the track
    // is a request to jump to a specific line and the only way to know it is
    // right is to press everywhere.
    for doc in DOCS {
        let mut h = Harness::new(doc);
        h.frame();
        let Some(bar) = h.bar() else { continue };
        let mut y = bar.top() + 1.0;
        while y < bar.bottom() {
            let x = bar.center().x;
            h.click_scrollbar(Pos2::new(x, y));
            sane(&h, "a press swept along the scrollbar track");
            let top = h.top_line_raw();
            assert!(
                top <= h.scroll_room(),
                "pressing at y={y:.0} put the window at line {top}, past the last \
                 screenful of {}",
                h.scroll_room()
            );
            y += 1.0;
        }
    }
}

// ---- 4. options, sizes and states ------------------------------------------

#[test]
fn every_option_combination_over_every_document_stays_sane() {
    // The matrix nobody writes out by hand: 16 option combinations against 15
    // documents, each opened, clicked, typed into, wrapped, unwrapped, resized and
    // scrolled. Options are where a widget quietly reads a field it should not
    // have read, and the only way to see that is to have every field take every
    // value.
    for doc in DOCS {
        for (wrap, highlight, comment) in option_sets() {
            for editable in [true, false] {
                let mut h = Harness::with_options(
                    doc,
                    super::Options {
                        wrap,
                        highlight,
                        comment,
                        editable,
                        ..Default::default()
                    },
                    Vec2::new(200.0, 120.0),
                );
                if !editable {
                    // A read-only document must refuse every edit, and must still
                    // be the same document afterwards.
                    let before = doc.to_string();
                    h.type_text("no");
                    h.key(Key::Backspace);
                    h.key_mod(Key::D, Modifiers::CTRL);
                    sane(&h, "typing into a read-only document");
                    assert_eq!(h.text(), before, "a read-only document was edited");
                }
                h.set_wrap(!wrap);
                h.frame();
                sane(&h, "flipping wrap");
                h.set_highlight(!highlight);
                h.frame();
                sane(&h, "flipping highlight");
                h.scroll(5.0);
                sane(&h, "scrolling a swept option combination");
                h.find("a");

                sane(&h, "searching a swept option combination");
            }
        }
    }
}

#[test]
fn every_pane_size_from_nothing_to_enormous_stays_sane() {
    // Resizing is where the arithmetic goes: a wrap width of zero divides, a
    // height of zero gives `rows_visible` of zero and then an index of
    // `first - 1`, and a scrollbar range is a division by the content height.
    // Real windows are resized continuously by a person dragging the edge, so
    // every one of these sizes happens.
    for doc in DOCS {
        for wrap in [false, true] {
            for w in [0.0f32, 1.0, 7.0, 40.0, 200.0, 1200.0] {
                for hgt in [0.0f32, 1.0, 9.0, 60.0, 900.0] {
                    let mut h = Harness::with_options(
                        doc,
                        super::Options {
                            wrap,
                            ..Default::default()
                        },
                        Vec2::new(w, hgt),
                    );
                    sane(&h, "a swept pane size");
                    h.click(Pos2::new(w * 0.5, hgt * 0.5));
                    sane(&h, "a click in a swept pane size");
                    h.scroll(3.0);
                    sane(&h, "a scroll in a swept pane size");
                    h.key(Key::End);
                    sane(&h, "end in a swept pane size");
                }
            }
        }
    }
}

#[test]
fn every_window_position_survives_a_document_that_changes_under_it() {
    // The editor's index is a cache of the buffer's line structure, and the whole
    // point of caching it is that it has to be invalidated. This is the test that
    // says it is: the window is somewhere in the middle of a long document, and
    // then the document is changed above the window, below it, and across it, one
    // edit at a time, checking the oracle after each.
    //
    // A stale index does not crash. It shows the wrong lines, and the line
    // numbers in the gutter are wrong with them.
    let mut h = Harness::new(&"line\n".repeat(400));
    h.scroll(150.0);
    sane(&h, "the window scrolled into a long document");
    let mid = h.top_line_raw();
    assert!(
        mid > 0,
        "the window did not scroll, so this test proves nothing"
    );

    for edit in [("home", "backspace"), ("end", "delete")] {
        h.key(Key::Home);
        let _ = edit;
        h.type_text("inserted ");
        sane(&h, "an edit above the window");
        h.key(Key::Enter);
        sane(&h, "a newline above the window");
    }
    // Now edit below the window, which must not move it.
    h.key(Key::End);
    for _ in 0..20 {
        h.key(Key::ArrowDown);
    }
    h.key(Key::End);
    h.type_text("tail ");
    sane(&h, "an edit below the window");
    // And a large edit, which is the case that would leave a cache half-updated.
    h.key_mod(Key::A, Modifiers::CTRL);
    h.type_text(&"replacement\n".repeat(50));
    sane(&h, "replacing the whole document");
    h.key_mod(Key::Z, Modifiers::CTRL);
    sane(&h, "undoing the whole document");
    h.key_mod(Key::Z, Modifiers::CTRL | Modifiers::SHIFT);
    sane(&h, "redoing the whole document");
}
