//! Mapping between document characters and the visible window.
//!
//! This is the part that is easy to get wrong and impossible to eyeball. A
//! layout job built from visible lines only contains those lines, so every
//! character index in it is *window-local*: zero is the first character of the
//! first shaped line, not the first character of the document. `Galley`'s
//! `cursor_from_pos` and `pos_from_cursor` speak only that dialect.
//!
//! So every hit test and every caret rectangle has to come through here. With
//! soft wrap on it gets subtler still, because one logical line can occupy
//! several visual rows, and the window's character count no longer lines up
//! with "one line".
//!
//! The whole type is deliberately free of egui so it can be tested directly.

/// Ties a shaped window back to the document it came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Window {
    /// Document character index of the window's first character.
    base_char: usize,
    /// Characters in the window, as the layout job counts them.
    len: usize,
    /// Stretches of the document that are *not* in the layout job, in order. A line
    /// far longer than the pane is shaped only where it is on screen, so the job
    /// holds a slice of it and these say where the rest would have been.
    gaps: Vec<Gap>,
}

/// Characters left out of the layout job: `n` of them, sitting just before local
/// position `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Gap {
    at: usize,
    n: usize,
}

impl Window {
    /// A window starting at document character `base_char`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(base_char: usize, len: usize) -> Window {
        Window {
            base_char,
            len,
            gaps: Vec::new(),
        }
    }

    /// A window that leaves out stretches of the document. Each gap is
    /// `(local position, characters omitted just before it)`, in order.
    pub fn with_gaps(base_char: usize, len: usize, gaps: Vec<(usize, usize)>) -> Window {
        Window {
            base_char,
            len,
            gaps: gaps
                .into_iter()
                .filter(|&(_, n)| n > 0)
                .map(|(at, n)| Gap { at, n })
                .collect(),
        }
    }

    /// Document character for a window-local one, clamped to the window.
    ///
    /// A caret one past the end is meaningful: it is the position after the
    /// last character, and clamping keeps it on the last row instead of
    /// dropping it off the end.
    pub fn to_document(&self, local: usize) -> usize {
        let local = local.min(self.len);
        let omitted: usize = self
            .gaps
            .iter()
            .take_while(|g| g.at <= local)
            .map(|g| g.n)
            .sum();
        self.base_char + local + omitted
    }

    /// The document character the window starts at.
    pub fn base(&self) -> usize {
        self.base_char
    }

    /// How many characters the layout job holds.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// How many document characters the window spans, counting the ones left out.
    pub fn doc_len(&self) -> usize {
        self.len + self.gaps.iter().map(|g| g.n).sum::<usize>()
    }

    /// Window-local character for a document one, or `None` when it is scrolled
    /// out of the window or sits in a stretch that was left out of it.
    pub fn to_local(&self, doc: usize) -> Option<usize> {
        let mut omitted = 0;
        for g in &self.gaps {
            let start = self.base_char + g.at + omitted;
            let end = start + g.n;
            if doc < start {
                break;
            }
            if doc < end {
                return None;
            }
            omitted += g.n;
        }
        let local = doc.checked_sub(self.base_char + omitted)?;
        (local <= self.len).then_some(local)
    }

    /// The window-local character nearest a document one: the ends of the window
    /// for something above or below it, and the edge of the slice for something in a
    /// stretch that was left out. For drawing a selection that runs off the window,
    /// which has to reach the edge and not vanish.
    pub fn to_local_clamped(&self, doc: usize) -> usize {
        let mut omitted = 0;
        for g in &self.gaps {
            let start = self.base_char + g.at + omitted;
            let end = start + g.n;
            if doc < start {
                break;
            }
            if doc < end {
                return g.at;
            }
            omitted += g.n;
        }
        doc.saturating_sub(self.base_char + omitted).min(self.len)
    }

    /// Which visual row a window-local character is on, given the rows the
    /// layout job produced.
    ///
    /// Without wrapping there is one row per line and this is the row whose
    /// range contains `local`. With wrapping the row boundaries come from the
    /// galley, so this is a scan of the ranges either way.
    ///
    /// A position that lands exactly on a newline belongs to no row's characters: it
    /// is the single character in the gap between two rows, because the layout
    /// consumes the newline as the row break rather than drawing it. The caret there
    /// is at the *end of the line before the newline*, so it is drawn at the end of
    /// the row that stops there. Not at the start of the row after the gap, which is
    /// the next line: that put the caret on the line below the one being typed on.
    ///
    /// A position where two rows meet with no gap between them is a soft wrap, and
    /// belongs to the row that starts there.
    pub fn row_of_local(&self, local: usize, rows: &[crate::codeedit::RowSpan]) -> usize {
        if let Some(i) = rows
            .iter()
            .position(|r| local >= r.chars.0 && local < r.chars.1.max(r.chars.0 + 1))
        {
            return i;
        }
        // A gap: the newline that ends a line, where the caret sits at that line's end.
        if let Some(i) = rows.iter().position(|r| r.chars.1 == local) {
            return i;
        }
        // Not at the end of any row: past the end of the window, or in a gap that
        // no row stops at. Shown on the row that follows the gap.
        //
        // Falling back to the last row instead put the caret at the bottom of the
        // window for every position in such a gap.
        rows.iter()
            .position(|r| local < r.chars.0)
            .unwrap_or_else(|| rows.len().saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_offsets_by_its_base() {
        let w = Window::new(100, 20);
        assert_eq!(w.to_document(0), 100, "the first shaped char");
        assert_eq!(w.to_document(5), 105);
        assert_eq!(w.to_local(105), Some(5));
    }

    #[test]
    fn local_indices_clamp_to_the_window_and_never_underflow() {
        let w = Window::new(100, 20);
        // One past the end is a real caret position, so it must survive.
        assert_eq!(w.to_document(20), 120);
        // Well past the end clamps rather than walking off into the document.
        assert_eq!(w.to_document(9999), 120);
        // Before the window: subtracting must not wrap around.
        assert_eq!(w.to_local(0), None);
        assert_eq!(w.to_local(99), None);
        assert_eq!(w.to_local(100), Some(0), "the first character is inside");
        assert_eq!(
            w.to_local(120),
            Some(20),
            "one past the end is still inside"
        );
    }

    #[test]
    fn a_document_at_the_base_of_a_scrolled_window_is_exactly_zero() {
        // This is the whole reason the type exists: after scrolling to line 900
        // of a document, the galley's cursor 0 is not character 0.
        let w = Window::new(45_000, 120);
        assert_eq!(w.to_document(0), 45_000);
        assert_eq!(w.to_local(0), None, "character 0 is off screen");
        assert_eq!(w.to_local(45_000), Some(0));
    }

    #[test]
    fn a_round_trip_through_the_window_preserves_the_index() {
        let base = 7;
        let w = Window::new(base, 50);
        for doc in base..=base + 50 {
            let local = w.to_local(doc).expect("inside");
            assert_eq!(w.to_document(local), doc, "round trip for {doc}");
        }
    }

    /// Three unwrapped rows of ten characters each.
    fn rows() -> Vec<crate::codeedit::RowSpan> {
        [(0usize, 10usize, 0usize), (10, 20, 1), (20, 30, 2)]
            .into_iter()
            .map(|(a, b, line)| crate::codeedit::RowSpan {
                chars: (a, b),
                line,
            })
            .collect()
    }

    #[test]
    fn the_row_of_a_character_is_found_by_its_range() {
        let rows = rows();
        assert_eq!(Window::new(0, 30).row_of_local(0, &rows), 0);
        assert_eq!(Window::new(0, 30).row_of_local(9, &rows), 0);
        assert_eq!(Window::new(0, 30).row_of_local(10, &rows), 1);
        assert_eq!(Window::new(0, 30).row_of_local(25, &rows), 2);
        // One past the end lands on the last row, not off the bottom.
        assert_eq!(Window::new(0, 30).row_of_local(30, &rows), 2);
    }

    #[test]
    fn a_wrapped_line_reports_several_rows_for_one_line() {
        // With wrap on, one logical line of 30 characters is three rows, so
        // "which row" cannot be answered from the line number alone. Every row
        // here belongs to the same line, which is the point.
        let rows: Vec<crate::codeedit::RowSpan> = [(0usize, 10usize), (10, 20), (20, 30)]
            .into_iter()
            .map(|(a, b)| crate::codeedit::RowSpan {
                chars: (a, b),
                line: 7,
            })
            .collect();
        let w = Window::new(500, 30);
        for (row, local) in [(0usize, 3usize), (1, 15), (2, 27)] {
            assert_eq!(w.row_of_local(local, &rows), row);
        }
        assert!(
            rows.iter().all(|r| r.line == 7),
            "three rows, one line, and the gutter must number it once"
        );
    }

    #[test]
    fn an_empty_row_list_still_answers_without_panicking() {
        let w = Window::new(0, 0);
        assert_eq!(w.row_of_local(0, &[]), 0, "no rows, so the first of none");
        assert_eq!(w.to_document(0), 0);
    }
}
