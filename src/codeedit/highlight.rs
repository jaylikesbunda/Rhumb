//! What state each line starts in, kept so a line can be coloured on its own.
//!
//! A block comment or a triple-quoted string opens on one line and closes on a
//! later one, so colouring line 4,000 needs to know what the 3,999 lines above it
//! left open. Scanning them every frame would put the whole file back into every
//! frame. So the state at the start of each line is remembered as it is worked
//! out, and only ever worked out once: the lines the window shows are scanned
//! from the state they are looked up in.
//!
//! An edit can only change the state of the lines *after* it, so the memory is cut
//! back to the edited line and rebuilt lazily, and only as far down as the window
//! needs. An edit below the window costs nothing at all.
//!
//! When the window is far below what is known — opening a large file at the end,
//! or dragging the scrollbar there — the scan is handed to a thread over a copy of
//! the rope, which is a pointer bump. Until it answers the caller draws the text
//! uncoloured for a few frames instead of holding the window still.

use super::markup::{Lang, State, advance};
use crate::buffer::Buffer;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Lines scanned on the calling thread before the rest is handed to a worker. A
/// screenful is about fifty; this is a few milliseconds of scanning at most.
const SYNC_LINES: usize = 20_000;

/// Lines the worker scans between handing results back.
const BATCH: usize = 8_192;

/// What the worker has found so far.
#[derive(Default)]
struct Shared {
    /// `states[k]` is the state at the start of line `base + k`.
    states: Vec<State>,
}

/// A scan running on another thread.
struct Job {
    shared: Arc<Mutex<Shared>>,
    /// The line the first state in `shared` belongs to.
    base: usize,
}

/// The remembered start state of every line scanned so far.
#[derive(Default)]
pub struct Highlights {
    /// `starts[i]` is the state at the start of line `i`.
    starts: Vec<State>,
    lang: Option<Lang>,
    /// Bumped whenever the text changes, so a worker can tell its copy is old.
    generation: Arc<AtomicU64>,
    job: Option<Job>,
}

impl Drop for Highlights {
    fn drop(&mut self) {
        // A worker for a document that is no longer open has nothing left to do.
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl Highlights {
    /// The state at the start of `line`, or `None` while a worker is still finding
    /// it out.
    pub fn state_at(
        &mut self,
        text: &Buffer,
        lang: &Lang,
        line: usize,
        ctx: &egui::Context,
    ) -> Option<State> {
        if self.lang != Some(*lang) {
            self.forget();
            self.lang = Some(*lang);
        }
        if self.starts.is_empty() {
            self.starts.push(State::Normal);
        }
        self.merge();
        if line < self.starts.len() {
            return Some(self.starts[line]);
        }
        if self.job.is_none() {
            let gap = line + 1 - self.starts.len();
            if gap <= SYNC_LINES {
                // Close enough to do here, before the frame is drawn.
                let mut state = *self.starts.last().unwrap_or(&State::Normal);
                for l in self.starts.len() - 1..line {
                    state = advance(&text.line_str(l), lang, state);
                    self.starts.push(state);
                }
                return Some(self.starts[line]);
            }
            self.spawn(text, lang, ctx);
        }
        None
    }

    /// Something in `line` or below it changed, so what is remembered about the
    /// lines after it is no longer true.
    pub fn invalidate_from(&mut self, line: usize) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.job = None;
        self.starts.truncate(line + 1);
    }

    /// How many lines have a remembered state, for the tests.
    #[cfg(test)]
    pub fn known(&self) -> usize {
        self.starts.len()
    }

    fn forget(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.job = None;
        self.starts.clear();
    }

    /// Takes in whatever the worker has produced since the last look.
    fn merge(&mut self) {
        let Some(job) = &self.job else { return };
        let done = {
            let Ok(shared) = job.shared.lock() else {
                return;
            };
            let have = self.starts.len().saturating_sub(job.base);
            if shared.states.len() > have {
                self.starts.extend_from_slice(&shared.states[have..]);
            }
            Arc::strong_count(&job.shared) == 1
        };
        // The worker lets go of its half when it finishes, which is how a finished
        // scan is told from one that is merely between batches.
        if done {
            self.job = None;
        }
    }

    /// Starts scanning from the first line nobody knows the state of.
    fn spawn(&mut self, text: &Buffer, lang: &Lang, ctx: &egui::Context) {
        let base = self.starts.len();
        let from = base - 1;
        let start = self.starts[from];
        let shared = Arc::new(Mutex::new(Shared::default()));
        self.job = Some(Job {
            shared: Arc::clone(&shared),
            base,
        });
        let snapshot = text.snapshot();
        let lang = *lang;
        let generation = Arc::clone(&self.generation);
        let wanted = generation.load(Ordering::Relaxed);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let last = snapshot.lines().saturating_sub(1);
            let mut state = start;
            let mut line = from;
            let mut batch = Vec::with_capacity(BATCH);
            while line < last {
                if generation.load(Ordering::Relaxed) != wanted {
                    return;
                }
                let stop = (line + BATCH).min(last);
                while line < stop {
                    state = advance(&snapshot.line_str(line), &lang, state);
                    batch.push(state);
                    line += 1;
                }
                if let Ok(mut s) = shared.lock() {
                    s.states.append(&mut batch);
                }
                ctx.request_repaint();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Lang = Lang {
        line: "//",
        block: Some(("/*", "*/")),
        triple: false,
        template: false,
    };

    fn ctx() -> egui::Context {
        egui::Context::default()
    }

    #[test]
    fn a_block_comment_colours_the_lines_it_spans() {
        let text = Buffer::from("a /* start\nstill comment\nend */ b\nplain");
        let mut h = Highlights::default();
        let c = ctx();
        let at = |h: &mut Highlights, l| h.state_at(&text, &C, l, &c);
        assert_eq!(at(&mut h, 0), Some(State::Normal));
        assert_eq!(at(&mut h, 1), Some(State::Block));
        assert_eq!(at(&mut h, 2), Some(State::Block));
        assert_eq!(at(&mut h, 3), Some(State::Normal));
    }

    #[test]
    fn an_edit_changes_only_the_lines_after_it() {
        let mut text = Buffer::from("one\ntwo\nthree\nfour");
        let mut h = Highlights::default();
        let c = ctx();
        assert_eq!(h.state_at(&text, &C, 3, &c), Some(State::Normal));
        assert_eq!(h.known(), 4);
        // Open a comment on line 1: lines 2 and 3 are now inside it.
        text.insert(text.line_start(1), "/* ");
        let from = text.take_dirty_line().expect("an edit marks its line");
        assert_eq!(from, 1);
        h.invalidate_from(from);
        assert_eq!(h.known(), 2, "what is above the edit is kept");
        assert_eq!(h.state_at(&text, &C, 2, &c), Some(State::Block));
        assert_eq!(h.state_at(&text, &C, 3, &c), Some(State::Block));
    }

    #[test]
    fn a_far_line_is_worked_out_on_a_worker_and_arrives_later() {
        // More lines than are scanned on the calling thread.
        let mut src = String::from("/* opens here\n");
        src.push_str(&"filler line\n".repeat(SYNC_LINES * 3));
        src.push_str("*/ closes\nafter");
        let text = Buffer::from(src.as_str());
        let last = text.lines() - 1;
        let mut h = Highlights::default();
        let c = ctx();
        assert_eq!(
            h.state_at(&text, &C, last, &c),
            None,
            "too far to answer at once"
        );
        let mut got = None;
        for _ in 0..2000 {
            got = h.state_at(&text, &C, last, &c);
            if got.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            got,
            Some(State::Normal),
            "the comment closed before the end"
        );
        // The line holding the `*/` starts inside the comment and ends outside it.
        assert_eq!(h.state_at(&text, &C, last - 1, &c), Some(State::Block));
        assert_eq!(h.state_at(&text, &C, 5, &c), Some(State::Block));
    }

    #[test]
    fn an_edit_while_a_worker_runs_throws_its_answer_away() {
        let mut src = String::new();
        src.push_str(&"filler line\n".repeat(SYNC_LINES * 3));
        src.push_str("end");
        let mut text = Buffer::from(src.as_str());
        let last = text.lines() - 1;
        let mut h = Highlights::default();
        let c = ctx();
        assert_eq!(h.state_at(&text, &C, last, &c), None);
        // An edit at the top that opens a comment nobody closes.
        text.insert(0, "/* ");
        let from = text.take_dirty_line().unwrap();
        h.invalidate_from(from);
        let mut got = None;
        for _ in 0..2000 {
            got = h.state_at(&text, &C, last, &c);
            if got.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            got,
            Some(State::Block),
            "the answer is for the text as it is now"
        );
    }
}
