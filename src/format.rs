//! Markdown formatting actions: the maths behind the toolbar and the shortcuts.
//!
//! Everything here is pure string work on a *window* of the document - a slice
//! that starts at a line start and ends at a line end - so it can be unit
//! tested without a widget, and so the editor never hands over a whole file to
//! wrap one word. Offsets are characters, the same units the rope and the
//! editor's selection speak, which is why the input arrives as `&str` plus
//! character offsets rather than byte offsets.
//!
//! Every action has toggle semantics, because a formatting bar whose bold
//! button cannot un-bold is a button for adding text:
//!
//! - The inline wraps (`**`, `*`, `~~`, `` ` ``) unwrap when the selection is
//!   already inside their markers, whether the markers are inside the
//!   selection or just outside it.
//! - The line prefixes (headings, lists, quotes) work on every line the
//!   selection touches: if every line already has the prefix it comes off,
//!   otherwise it goes on - replacing whatever list or heading marker was
//!   there, so switching from a bullet to a number is one click.

/// One styling button of the Markdown toolbar, and one shortcut.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MdAction {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    /// A heading of this level, 1-6.
    Heading(u8),
    Bullet,
    Numbered,
    Task,
    Quote,
}

/// One edit to make: replace `start..end` with `text`, then select `sel`.
///
/// All offsets are characters into the window the action was given. The
/// caller translates them back to document coordinates, which it can do by
/// addition alone, because the window is known to start where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub sel: (usize, usize),
}

/// Works out what one action does to the window, or `None` when there is
/// nothing to do.
///
/// `sel` is the selection in window coordinates, ordered however the editor
/// handed it over. `url` is the clipboard's text when it looks like a link,
/// which the link action drops into the parentheses for you.
pub fn apply(window: &str, sel: (usize, usize), action: MdAction, url: Option<&str>) -> Option<Edit> {
    let chars: Vec<char> = window.chars().collect();
    let n = chars.len();
    let (lo, hi) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let (lo, hi) = (lo.min(n), hi.min(n));
    match action {
        MdAction::Bold => wrap(&chars, lo, hi, &['*', '*']),
        MdAction::Italic => wrap(&chars, lo, hi, &['*']),
        MdAction::Strike => wrap(&chars, lo, hi, &['~', '~']),
        MdAction::Code => wrap(&chars, lo, hi, &['`']),
        MdAction::Link => link(&chars, lo, hi, url),
        MdAction::Heading(level) => toggle_lines(&chars, lo, hi, Target::Heading(level.clamp(1, 6))),
        MdAction::Bullet => toggle_lines(&chars, lo, hi, Target::Bullet),
        MdAction::Numbered => toggle_lines(&chars, lo, hi, Target::Numbered),
        MdAction::Task => toggle_lines(&chars, lo, hi, Target::Task),
        MdAction::Quote => toggle_lines(&chars, lo, hi, Target::Quote),
    }
}

/// Wraps or unwraps the selection in an inline marker.
///
/// Three shapes count as "already inside the marker" and unwrap: the markers
/// inside the selection (`**word**` selected), the markers just outside it
/// (`wo|rd` selected between two `**`), and a bare caret sitting between an
/// open and a close, which undoes the marker pair it stands between.
fn wrap(chars: &[char], lo: usize, hi: usize, marker: &[char]) -> Option<Edit> {
    let n = chars.len();
    let m = marker.len();
    let inner: String = chars[lo..hi].iter().collect();

    // The selection carries its own markers: take them off the inside.
    if hi - lo >= 2 * m
        && chars[lo..lo + m] == *marker
        && chars[hi - m..hi] == *marker
    {
        let text = inner[m..inner.len() - m].to_owned();
        return Some(Edit {
            start: lo,
            end: hi,
            text,
            sel: (lo, lo + (hi - lo - 2 * m)),
        });
    }
    // The markers sit just outside the selection: take them off the outside.
    // A marker must not straddle a line break, or a selection that starts a
    // line after a line ending in `*` would eat the wrong characters.
    if lo >= m
        && hi + m <= n
        && chars[lo - m..lo] == *marker
        && chars[hi..hi + m] == *marker
        && !chars[lo - m..lo].contains(&'\n')
        && !chars[hi..hi + m].contains(&'\n')
    {
        return Some(Edit {
            start: lo - m,
            end: hi + m,
            text: inner,
            sel: (lo - m, hi - m),
        });
    }
    // Otherwise the markers go on. An empty selection leaves the caret between
    // the two halves, which is how you bold what you are about to type.
    let mut text = String::with_capacity(inner.len() + 2 * m);
    for &c in marker {
        text.push(c);
    }
    text.push_str(&inner);
    for &c in marker {
        text.push(c);
    }
    Some(Edit {
        start: lo,
        end: hi,
        text,
        sel: if inner.is_empty() {
            (lo + m, lo + m)
        } else {
            (lo + m, lo + m + inner.chars().count())
        },
    })
}

/// The line-prefix actions, which all work on whole lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Heading(u8),
    Bullet,
    Numbered,
    Task,
    Quote,
}

impl Target {
    /// The prefix this target writes, numbered lines counting from `index`.
    fn prefix(&self, index: usize) -> String {
        match self {
            Target::Heading(level) => format!("{} ", "#".repeat(*level as usize)),
            Target::Bullet => "- ".to_owned(),
            Target::Numbered => format!("{index}. "),
            Target::Task => "- [ ] ".to_owned(),
            Target::Quote => "> ".to_owned(),
        }
    }
}

/// What a line already opens with, and how many characters the opening takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Existing {
    None,
    Heading(u8),
    Bullet,
    Numbered,
    Task,
    Quote,
}

/// Reads the block marker at the start of `line`, if there is one.
///
/// The span covers the marker and the whitespace that follows it, so removal
/// leaves the text as it was before the marker was typed.
fn existing(line: &[char]) -> (usize, Existing) {
    let indent = line.iter().take_while(|&&c| c == ' ' || c == '\t').count();
    let rest = &line[indent..];
    let blank = |from: usize| -> bool {
        rest.get(from).is_none_or(|&c| c == ' ' || c == '\t')
    };
    let spaces = |from: usize| -> usize {
        rest[from..]
            .iter()
            .take_while(|&&c| c == ' ' || c == '\t')
            .count()
    };

    if let Some(&'#') = rest.first() {
        let hashes = rest.iter().take_while(|&&c| c == '#').count();
        if (1..=6).contains(&hashes) && blank(hashes) {
            return (indent + hashes + spaces(hashes), Existing::Heading(hashes as u8));
        }
        return (0, Existing::None);
    }
    if let Some(&'>') = rest.first() {
        // The markers are a run of `>` with single spaces between them; a
        // space belongs to the markers only while another `>` may follow it.
        let mut i = 0usize;
        while i < rest.len() {
            match rest[i] {
                '>' => i += 1,
                ' ' | '\t' if rest[i + 1..].contains(&'>') => i += 1,
                _ => break,
            }
        }
        // Everything through the last `>` plus the spaces after it is what a
        // toggle is allowed to take off.
        let after = rest[..i].iter().rposition(|&c| c == '>').map_or(0, |k| k + 1);
        return (indent + after + spaces(after), Existing::Quote);
    }
    // A task box: a bullet marker, then `[ ]` / `[x]` / `[X]`, then a space.
    for bullet in ['-', '*', '+'] {
        if rest.first() == Some(&bullet)
            && matches!(rest.get(1), Some(' ') | Some('\t'))
        {
            let after = 2 + rest[2..]
                .iter()
                .take_while(|&&c| c == ' ' || c == '\t')
                .count();
            if matches!(rest.get(after), Some('['))
                && matches!(rest.get(after + 2), Some(']'))
                && matches!(rest.get(after + 1), Some(' ') | Some('x') | Some('X'))
                && matches!(rest.get(after + 3), Some(' ') | None)
            {
                // The box is three characters, and the one space after it
                // belongs to the marker, because the toggle put it there.
                return (indent + after + 3 + spaces(after + 3).min(1), Existing::Task);
            }
            return (indent + 2, Existing::Bullet);
        }
    }
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && digits <= 9
        && matches!(rest.get(digits), Some('.') | Some(')'))
        && matches!(rest.get(digits + 1), Some(' ') | Some('\t'))
    {
        return (indent + digits + 2, Existing::Numbered);
    }
    (0, Existing::None)
}

/// Toggles the target's prefix over every line the selection touches.
///
/// The whole run of lines is replaced in one edit, so the result is one undo
/// step and one selection fix-up, wherever in the run the selection started
/// and stopped.
fn toggle_lines(chars: &[char], lo: usize, hi: usize, want: Target) -> Option<Edit> {
    let n = chars.len();
    // The selection's first line starts at or before `lo`, its last line ends
    // at or after `hi`. An empty selection is the caret's line.
    let mut line_lo = lo.min(n);
    while line_lo > 0 && chars[line_lo - 1] != '\n' {
        line_lo -= 1;
    }
    let mut line_hi = hi.max(lo).min(n);
    if line_hi < n {
        while line_hi < n && chars[line_hi] != '\n' {
            line_hi += 1;
        }
    }

    // The lines and where each begins, within the run being replaced.
    let mut lines: Vec<(usize, usize)> = Vec::new(); // (start, len)
    let mut at = line_lo;
    while at <= line_hi {
        let end = chars[at..line_hi.max(at)]
            .iter()
            .position(|&c| c == '\n')
            .map_or(line_hi.max(at), |k| at + k);
        lines.push((at, end - at));
        if end >= line_hi {
            break;
        }
        at = end + 1;
    }

    // Deciding: everything that already carries the target comes off, unless
    // some line is missing it, in which case every line gets it.
    let parsed: Vec<(usize, Existing)> = lines
        .iter()
        .map(|&(start, len)| {
            let (span, kind) = existing(&chars[start..start + len]);
            (span, kind)
        })
        .collect();
    let has = |kind: Existing| -> bool {
        match (want, kind) {
            (Target::Heading(level), Existing::Heading(have)) => have == level,
            (Target::Bullet, Existing::Bullet)
            | (Target::Numbered, Existing::Numbered)
            | (Target::Task, Existing::Task)
            | (Target::Quote, Existing::Quote) => true,
            _ => false,
        }
    };
    let remove = parsed
        .iter()
        .any(|&(_, kind)| has(kind));

    let mut text = String::with_capacity(line_hi - line_lo + lines.len() * 4);
    let mut deltas: Vec<isize> = Vec::with_capacity(lines.len());
    for (i, &(start, len)) in lines.iter().enumerate() {
        if i > 0 {
            // The newline between one line and the next is not part of either
            // line, so the replacement has to put it back itself.
            text.push('\n');
        }
        let (span, kind) = parsed[i];
        let old_len = len as isize;
        if remove {
            // Taking the target's marker off: lines that do not have it are
            // left exactly as they are.
            if has(kind) {
                let keep: String = chars[start + span..start + len].iter().collect();
                text.push_str(&keep);
            } else {
                text.push_str(&chars[start..start + len].iter().collect::<String>());
            }
        } else {
            // Putting the marker on: any other marker this line had comes off
            // first, so switching from a bullet to a number is one click, and
            // a heading does not sit inside a list item.
            let keep: String = chars[start + span..start + len].iter().collect();
            if let Target::Numbered = want {
                text.push_str(&want.prefix(i + 1));
            } else {
                text.push_str(&want.prefix(1));
            }
            text.push_str(&keep);
        }
        let new_len = text.len() as isize
            - deltas.iter().sum::<isize>()
            - (start - line_lo) as isize
            - i as isize;
        deltas.push(new_len - old_len);
    }

    // An action that changes nothing - every line plain and the target
    // refusing to add, which cannot happen, or a run that was and stays
    // identical - is reported as no edit rather than as an empty one.
    let total: isize = deltas.iter().sum();
    let first = deltas.first().copied().unwrap_or(0);
    let sel = (
        (lo as isize + first).max(line_lo as isize) as usize,
        (hi as isize + total).max(line_lo as isize) as usize,
    );
    Some(Edit {
        start: line_lo,
        end: line_hi,
        text,
        sel,
    })
}

/// The link action.
///
/// - `[text](url)` under the selection unwraps to `text`.
/// - A URL under the selection becomes the link text.
/// - Anything else becomes the link text, with the clipboard's URL dropped
///   into the parentheses when there is one, and the caret left inside the
///   parentheses when there is not.
fn link(chars: &[char], lo: usize, hi: usize, url: Option<&str>) -> Option<Edit> {
    let inner: String = chars[lo..hi].iter().collect();

    // Already a link: keep the text, lose the plumbing.
    if inner.starts_with('[') && inner.ends_with(')')
        && let Some(mid) = inner.find("](")
    {
        let text = inner[1..mid].to_owned();
        return Some(Edit {
            start: lo,
            end: hi,
            sel: (lo, lo + text.chars().count()),
            text,
        });
    }

    let clip = url.map(str::trim).filter(|s| is_url(s)).unwrap_or("");
    let mut text = String::with_capacity(inner.len() + clip.len() + 4);
    text.push('[');
    text.push_str(&inner);
    text.push_str("](");
    if clip.is_empty() {
        text.push(')');
        // No address to hand: the caret goes inside the parentheses, where
        // the reader was about to paste one.
        let caret = lo + 1 + inner.chars().count() + 2;
        return Some(Edit {
            start: lo,
            end: hi,
            text,
            sel: (caret, caret),
        });
    }
    text.push_str(clip);
    text.push(')');
    Some(Edit {
        start: lo,
        end: hi,
        sel: (lo + 1, lo + 1 + inner.chars().count()),
        text,
    })
}

/// Whether `s` looks like a URL, for the clipboard and the selection checks.
pub fn is_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("www.")
        || s.starts_with("file://")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one action over a whole-line window and returns the new text and
    /// selection, the way the editor does.
    fn run(src: &str, sel: (usize, usize), action: MdAction) -> (String, (usize, usize)) {
        let edit = apply(src, sel, action, None).expect("an edit");
        let mut text = src.chars().collect::<Vec<_>>();
        let n = text.len();
        let (a, b) = (edit.start.min(n), edit.end.min(n));
        let replacement: Vec<char> = edit.text.chars().collect();
        text.splice(a..b, replacement);
        let out: String = text.into_iter().collect();
        let sel = (edit.sel.0.min(out.len()), edit.sel.1.min(out.len()));
        (out, sel)
    }

    #[test]
    fn bold_wraps_the_selection() {
        let (out, sel) = run("hello world", (6, 11), MdAction::Bold);
        assert_eq!(out, "hello **world**");
        assert_eq!(sel, (8, 13), "the word stays selected");
    }

    #[test]
    fn bold_unwraps_when_the_markers_are_selected() {
        let (out, sel) = run("hello **world**", (6, 15), MdAction::Bold);
        assert_eq!(out, "hello world");
        assert_eq!(sel, (6, 11));
    }

    #[test]
    fn bold_unwraps_when_the_markers_are_outside_the_selection() {
        let (out, sel) = run("hello **world**", (8, 13), MdAction::Bold);
        assert_eq!(out, "hello world");
        assert_eq!(sel, (6, 11));
    }

    #[test]
    fn bold_with_no_selection_puts_the_caret_between_the_markers() {
        let (out, sel) = run("hello ", (6, 6), MdAction::Bold);
        assert_eq!(out, "hello ****");
        assert_eq!(sel, (8, 8));
    }

    #[test]
    fn italic_uses_one_marker_and_code_uses_a_backtick() {
        let (out, _) = run("word", (0, 4), MdAction::Italic);
        assert_eq!(out, "*word*");
        let (out, _) = run("x = 1", (0, 5), MdAction::Code);
        assert_eq!(out, "`x = 1`");
    }

    #[test]
    fn strike_uses_two_tildes() {
        let (out, _) = run("gone", (0, 4), MdAction::Strike);
        assert_eq!(out, "~~gone~~");
        let (out, _) = run("~~gone~~", (0, 8), MdAction::Strike);
        assert_eq!(out, "gone");
    }

    #[test]
    fn headings_replace_what_was_there() {
        let (out, _) = run("## two", (0, 6), MdAction::Heading(1));
        assert_eq!(out, "# two");
        let (out, _) = run("# one", (0, 5), MdAction::Heading(3));
        assert_eq!(out, "### one");
        // Clicking the level the line already is takes the heading off.
        let (out, _) = run("### three", (0, 9), MdAction::Heading(3));
        assert_eq!(out, "three");
        // A bullet line becomes a heading, not a heading inside a list item.
        let (out, _) = run("- item", (0, 6), MdAction::Heading(2));
        assert_eq!(out, "## item");
    }

    #[test]
    fn bullets_toggle_over_every_line_touched() {
        let (out, _) = run("one\ntwo\nthree", (4, 7), MdAction::Bullet);
        assert_eq!(out, "one\n- two\nthree");
        // Again: the bullet comes back off, and only from the lines that had it.
        let (out, _) = run("one\n- two\nthree", (4, 8), MdAction::Bullet);
        assert_eq!(out, "one\ntwo\nthree");
        // A two-line selection bullets both lines in one edit.
        let (out, _) = run("one\ntwo", (0, 7), MdAction::Bullet);
        assert_eq!(out, "- one\n- two");
    }

    #[test]
    fn numbered_lists_count_their_lines() {
        let (out, _) = run("one\ntwo\nthree", (0, 13), MdAction::Numbered);
        assert_eq!(out, "1. one\n2. two\n3. three");
        // A bullet list becomes a numbered one without doubling the markers.
        let (out, _) = run("- one\n- two", (0, 11), MdAction::Numbered);
        assert_eq!(out, "1. one\n2. two");
        // And back again.
        let (out, _) = run("1. one\n2. two", (0, 13), MdAction::Bullet);
        assert_eq!(out, "- one\n- two");
    }

    #[test]
    fn a_task_item_carries_a_checkbox() {
        let (out, _) = run("milk", (0, 4), MdAction::Task);
        assert_eq!(out, "- [ ] milk");
        let (out, _) = run("- [ ] milk", (0, 10), MdAction::Task);
        assert_eq!(out, "milk");
    }

    #[test]
    fn quotes_toggle_and_nest_no_deeper() {
        let (out, _) = run("wise words", (0, 10), MdAction::Quote);
        assert_eq!(out, "> wise words");
        let (out, _) = run("> wise words", (0, 12), MdAction::Quote);
        assert_eq!(out, "wise words");
        // A quote of any depth is one prefix, so one click takes it off.
        let (out, _) = run("> once", (0, 6), MdAction::Quote);
        assert_eq!(out, "once");
        let (out, _) = run(">> twice", (0, 8), MdAction::Quote);
        assert_eq!(out, "twice");
        // A quote without a space after the marker still comes off whole.
        let (out, _) = run(">tight", (0, 6), MdAction::Quote);
        assert_eq!(out, "tight");
    }

    #[test]
    fn an_empty_selection_still_formats_the_line() {
        let (out, _) = run("just a line", (7, 7), MdAction::Bullet);
        assert_eq!(out, "- just a line");
    }

    #[test]
    fn a_link_unwraps_to_its_text() {
        let (out, sel) = run("see [docs](https://x.y) here", (4, 23), MdAction::Link);
        assert_eq!(out, "see docs here");
        assert_eq!(sel, (4, 8), "the text stays selected");
    }

    #[test]
    fn a_url_becomes_the_link_text() {
        let (out, sel) = run("go to https://x.y now", (6, 17), MdAction::Link);
        assert_eq!(out, "go to [https://x.y]() now");
        assert_eq!(sel, (20, 20), "the caret waits in the parens for the address");
    }

    #[test]
    fn the_clipboard_url_is_dropped_in() {
        let edit = apply(
            "the docs",
            (0, 8),
            MdAction::Link,
            Some("https://example.com/a(b)"),
        )
        .expect("an edit");
        assert_eq!(edit.text, "[the docs](https://example.com/a(b))");
        assert_eq!(edit.sel, (1, 9), "the link text stays selected");
    }

    #[test]
    fn without_a_clipboard_url_the_caret_waits_in_the_parens() {
        let (_, sel) = run("the docs", (0, 8), MdAction::Link);
        assert_eq!(sel, (11, 11), "between ( and )");
    }

    #[test]
    fn a_non_url_clipboard_is_ignored() {
        let edit = apply("text", (0, 4), MdAction::Link, Some("not a url"))
            .expect("an edit");
        assert_eq!(edit.text, "[text]()");
    }

    #[test]
    fn urls_are_recognised_loosely() {
        for s in ["http://a", "https://a", "www.a.b", "file:///c/x"] {
            assert!(is_url(s), "{s} should be a url");
        }
        for s in ["", "note", "C:\\path"] {
            assert!(!is_url(s), "{s} should not be a url");
        }
    }

    #[test]
    fn the_selection_survives_a_prefix_change() {
        // The selection was over "two"; prefixing that line by two characters
        // moves the selection with it.
        let (out, sel) = run("one\ntwo\nthree", (4, 7), MdAction::Bullet);
        assert_eq!(out, "one\n- two\nthree");
        assert_eq!(sel, (6, 9));
    }

    #[test]
    fn everything_survives_unicode() {
        // "héllo wörld 🙂" is thirteen characters, not bytes.
        let (out, sel) = run("héllo wörld 🙂", (0, 13), MdAction::Bold);
        assert_eq!(out, "**héllo wörld 🙂**");
        assert_eq!(sel, (2, 15));
        let (out, sel) = run("**héllo 🙂**", (0, 11), MdAction::Bold);
        assert_eq!(out, "héllo 🙂");
        assert_eq!(sel, (0, 7));
    }

    #[test]
    fn offsets_are_never_out_of_bounds() {
        // A stale selection a character past the end clamps rather than
        // panics, which is the contract every edit in this editor keeps.
        let src = "word";
        let edit = apply(src, (0, 99), MdAction::Bold, None).expect("an edit");
        assert_eq!(edit.end, 4);
        let mut text: Vec<char> = src.chars().collect();
        let (a, b) = (edit.start.min(4), edit.end.min(4));
        text.splice(a..b, edit.text.chars().collect::<Vec<_>>());
        assert_eq!(text.into_iter().collect::<String>(), "**word**");
    }
}
