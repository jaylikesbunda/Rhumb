//! Which lines fold, and what a fold hides.
//!
//! Two shapes are foldable, and they are found by two separate rules because
//! they are two different ideas:
//!
//! - **Bracket blocks.** A line whose last non-blank character is `{`, `[` or
//!   `(`, whose matching closer is on a later line. This is language-agnostic on
//!   purpose: it is right for Rust, C, JavaScript and JSON, and it does the
//!   sensible thing on a language nobody has taught it.
//! - **Indentation blocks.** A non-blank line followed by a more-indented line,
//!   and the run of more-indented lines after it. This is what folds a language
//!   without braces, and it is the only thing that folds a Markdown list.
//!
//! A line's fold is worked out *on demand*, when the line is drawn, and
//! remembered until an edit makes it wrong. A whole-document scan on every edit
//! would be the exact cost the editor exists to avoid: on a hundred-thousand-line
//! file it is over a hundred milliseconds, which is most of a frame spent on text
//! nobody is looking at. The lazy walk costs what is on screen and keeps the
//! editor's rule - nothing here is O(document) per frame.
//!
//! [`compute`] still builds the whole list, for "fold all" and for the tests
//! below, because a command a reader asked for may look at the whole document;
//! a frame may not.

use crate::buffer::Buffer;
use std::collections::HashMap;

/// One foldable region: the line that opens it and the last line it hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    /// The line whose chevron toggles the region. It stays visible when the fold
    /// is closed.
    pub start: usize,
    /// The last line the region hides. Lines `start + 1..=end` are hidden; `end`
    /// is always greater than `start`.
    pub end: usize,
}

/// The folds lines have been found to open, remembered as they are looked at.
#[derive(Default)]
pub struct Folds {
    /// Line -> the end it hides, or `None` for a line that was looked at and
    /// opens none. Absent means "not looked at yet".
    ends: HashMap<usize, Option<usize>>,
    /// Whether any line is known to open a fold, so the gutter can ask cheaply.
    any: bool,
}

impl Folds {
    /// Whether any line is known to open a fold.
    pub fn is_empty(&self) -> bool {
        !self.any
    }

    /// The last line hidden by the fold `line` opens, or `None` when it opens
    /// none. Worked out once and remembered until [`Folds::clear`].
    pub fn end_of(&mut self, text: &Buffer, line: usize) -> Option<usize> {
        if let Some(&known) = self.ends.get(&line) {
            return known;
        }
        let content = text.line_str(line);
        let end = if let Some(open) = last_opener(&content) {
            brace_end(text, line, open, &content).or_else(|| indent_end(text, line, &content))
        } else {
            indent_end(text, line, &content)
        };
        self.remember(line, end);
        end
    }

    /// Looks at a line while it is being drawn, using the text already in hand.
    ///
    /// Returns the fold found, which is usually the line's own but may be the one
    /// the line *before* it opens: an indentation block can only be recognised
    /// once the line under it has been read, and by then the line that opens it
    /// has gone by. The caller records the pair either way.
    ///
    /// `content` is the line's text and `prev` the line before it with its text,
    /// both already read for the shaping, so the common case - a line that opens
    /// nothing - reads the document not at all.
    pub fn consider(
        &mut self,
        text: &Buffer,
        line: usize,
        content: &str,
        prev: Option<(usize, &str)>,
    ) -> Option<(usize, usize)> {
        if let Some(open) = last_opener(content) {
            let end = match self.ends.get(&line) {
                Some(&known) => known,
                None => {
                    let end = brace_end(text, line, open, content);
                    self.remember(line, end);
                    end
                }
            };
            if let Some(end) = end {
                return Some((line, end));
            }
        }
        // An indentation block is the line before opening one, if this line is
        // indented further than it. A line that already opens a bracket block is
        // not asked about again: its chevron is the bracket's.
        let (before, before_text) = prev?;
        if before + 1 != line || last_opener(before_text).is_some() {
            return None;
        }
        if before_text.trim().is_empty() || content.trim().is_empty() {
            return None;
        }
        if indent(content) <= indent(before_text) {
            return None;
        }
        let end = match self.ends.get(&before) {
            Some(&known) => known,
            None => {
                let end = indent_end(text, before, before_text);
                self.remember(before, end);
                end
            }
        };
        end.map(|end| (before, end))
    }

    /// Forgets every fold. An edit can change what any of them hides.
    pub fn clear(&mut self) {
        self.ends.clear();
        self.any = false;
    }

    fn remember(&mut self, line: usize, end: Option<usize>) {
        if end.is_some() {
            self.any = true;
        }
        self.ends.insert(line, end);
    }
}

/// Every fold in the document, in order and never overlapping.
///
/// A whole-document scan, kept for "fold all" and for the tests. The editor does
/// not call it: it looks lines up through [`Folds`] as they are drawn.
pub fn compute(text: &Buffer) -> Vec<Fold> {
    let lines = text.lines();
    let mut found: Vec<Fold> = Vec::new();
    // Per line, in order: how far it is indented, whether it has anything on it,
    // and its last non-blank character when that character is an opener.
    let mut indent: Vec<usize> = Vec::with_capacity(lines);
    let mut blank: Vec<bool> = Vec::with_capacity(lines);
    let mut opener: Vec<Option<char>> = Vec::with_capacity(lines);
    // A stack of open brackets, so a nested block's opener is matched to its own
    // closer rather than to the first one that turns up. A closer that does not
    // match the top of the stack is ignored rather than popping it: an unbalanced
    // document should cost a missed fold, not a wrong one.
    let mut stack: Vec<(char, usize)> = Vec::new();

    let mut line = 0usize;
    let mut lead = 0usize;
    let mut in_lead = true;
    let mut last_non_ws: Option<char> = None;
    // The rope's own chunks, so the walk is over contiguous slices rather than
    // through the tree one character at a time.
    for chunk in text.chunks() {
        for c in chunk.chars() {
            if c == '\n' {
                indent.push(lead);
                blank.push(last_non_ws.is_none());
                opener.push(last_non_ws.filter(|c| matches!(*c, '{' | '[' | '(')));
                line += 1;
                lead = 0;
                in_lead = true;
                last_non_ws = None;
                continue;
            }
            if in_lead {
                if c == ' ' || c == '\t' {
                    lead += 1;
                } else {
                    in_lead = false;
                }
            }
            // Only space, tab and carriage return are blank here. The Unicode
            // whitespace table is not worth its cost on every character.
            if c != ' ' && c != '\t' && c != '\r' {
                last_non_ws = Some(c);
            }
            match c {
                '{' | '[' | '(' => stack.push((c, line)),
                '}' | ']' | ')' => {
                    if let Some(&(open, at)) = stack.last()
                        && closer(open) == c
                    {
                        stack.pop();
                        // The opener's own line has ended by the time a closer on
                        // a later line is reached, so `opener[at]` is known.
                        if at < line && opener.get(at).copied().flatten() == Some(open) {
                            found.push(Fold {
                                start: at,
                                end: line,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // The last line, which no newline ends.
    indent.push(lead);
    blank.push(last_non_ws.is_none());
    opener.push(last_non_ws.filter(|c| matches!(*c, '{' | '[' | '(')));

    // Indentation blocks. A block runs from a line to the last line after it that
    // is indented further. Blank lines end a block rather than extending it: a
    // blank line has no indentation to compare, and guessing would make the fold
    // depend on what happens to be below it.
    for line in 0..lines.saturating_sub(1) {
        if blank.get(line).copied().unwrap_or(true) {
            continue;
        }
        let base = indent.get(line).copied().unwrap_or(0);
        let mut end = line;
        while end + 1 < lines {
            if blank.get(end + 1).copied().unwrap_or(true)
                || indent.get(end + 1).copied().unwrap_or(0) <= base
            {
                break;
            }
            end += 1;
        }
        if end > line {
            found.push(Fold { start: line, end });
        }
    }

    // One region per opening line: sorted by start and then by the larger region
    // first, so an overlapping or repeated region is swallowed by the one that
    // already covers it.
    found.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut regions: Vec<Fold> = Vec::with_capacity(found.len());
    for f in found {
        match regions.last_mut() {
            Some(last) if f.start <= last.end => last.end = last.end.max(f.end),
            _ => regions.push(f),
        }
    }
    regions
}

/// The last non-blank character of a line, when it is an opener.
fn last_opener(line: &str) -> Option<char> {
    line.trim_end()
        .chars()
        .last()
        .filter(|c| matches!(*c, '{' | '[' | '('))
}

/// The line that closes the bracket `line` ends with, if one does.
///
/// Walked from the opener itself rather than a line at a time: a bracket block is
/// usually short, and one walk over its characters is cheaper than asking the
/// rope for each of its lines.
fn brace_end(text: &Buffer, line: usize, open: char, content: &str) -> Option<usize> {
    let close = closer(open);
    // Characters before the opener on its own line, which is the last non-blank
    // one, so the walk starts at it.
    let skip = content.trim_end().chars().count().saturating_sub(1);
    let mut depth = 0i32;
    let mut at = line;
    for c in text.chars_at(text.line_start(line) + skip) {
        if c == '\n' {
            at += 1;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                // A closer on the opener's own line is not a block.
                return (at > line).then_some(at);
            }
        }
    }
    None
}

/// The last line of the indentation block `line` opens, if it opens one.
fn indent_end(text: &Buffer, line: usize, content: &str) -> Option<usize> {
    if content.trim().is_empty() {
        return None;
    }
    let lines = text.lines();
    if line + 1 >= lines {
        return None;
    }
    let base = indent(content);
    let mut end = line;
    while end + 1 < lines {
        let next = text.line_str(end + 1);
        if next.trim().is_empty() || indent(&next) <= base {
            break;
        }
        end += 1;
    }
    (end > line).then_some(end)
}

/// The closer for an opener, or the character itself when it is not one.
fn closer(open: char) -> char {
    match open {
        '{' => '}',
        '[' => ']',
        '(' => ')',
        other => other,
    }
}

/// How many leading spaces or tabs a line has.
fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(text: &str) -> Vec<(usize, usize)> {
        compute(&Buffer::from(text))
            .iter()
            .map(|f| (f.start, f.end))
            .collect()
    }

    #[test]
    fn a_brace_block_folds_to_its_matching_closer() {
        assert_eq!(
            starts("fn f() {\n    a\n    b\n}\n"),
            vec![(0, 3)],
            "the opener's line through the closer's line"
        );
    }

    #[test]
    fn a_closer_on_the_same_line_is_not_a_fold() {
        assert!(starts("let x = { 1 };\n").is_empty());
    }

    #[test]
    fn an_indentation_block_folds_without_any_braces() {
        assert_eq!(starts("root\n  one\n  two\nsibling\n"), vec![(0, 2)]);
    }

    #[test]
    fn a_nested_block_is_swallowed_by_the_block_that_contains_it() {
        // The inner brace block and the outer one both start at line 0, and the
        // outer one reaches further, so there is a single chevron.
        assert_eq!(
            starts("outer {\n  inner {\n    x\n  }\n}\n"),
            vec![(0, 4)],
            "one region per opening line"
        );
    }

    #[test]
    fn a_bracket_that_does_not_end_its_line_is_not_a_fold() {
        // The `(` is matched on line 1, but the line it opened on does not end in
        // an opener, so it is not a block a reader would expect to fold. The
        // second line is flush left so that only the bracket pass is being asked.
        assert!(starts("let x = (1\n+ 2);\n").is_empty());
    }

    #[test]
    fn the_lazy_look_up_agrees_with_the_whole_document_scan() {
        // The editor finds folds a line at a time; `compute` finds them all at
        // once and merges the nested ones into the block that contains them. So
        // the two need not be identical - a nested line may keep its own chevron
        // in the editor - but every lazy fold must sit inside a region the scan
        // found, and every region the scan found must still be opened by its own
        // line. A disagreement the other way would be a chevron for a region that
        // is not there.
        let text = Buffer::from(
            "fn f() {\n    let a = 1;\n    if a {\n        b\n    }\n}\nroot\n  child\n",
        );
        let all = compute(&text);
        let mut lazy = Folds::default();
        for line in 0..text.lines() {
            if let Some(end) = lazy.end_of(&text, line) {
                assert!(
                    all.iter().any(|f| f.start <= line && f.end >= end),
                    "lazy found ({line}, {end}) that no scanned region contains"
                );
            }
        }
        for f in &all {
            assert_eq!(
                lazy.end_of(&text, f.start),
                Some(f.end),
                "the scan found ({}, {}) that the lazy look-up did not",
                f.start,
                f.end
            );
        }
    }
}
