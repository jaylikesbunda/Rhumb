//! Colouring one line of Markdown.
//!
//! The code scanner in [`super::markup`] is driven by a `Lang` that describes
//! comments and strings; Markdown shares nothing with that grammar, so it gets
//! its own scan here and the dispatch happens in `tokenize_with`, one `if` on
//! `Lang::md`. Everything downstream - the per-line state cache, the background
//! worker, the long-line guards - is written against `State` and never learns
//! the difference.
//!
//! The rule inherited from the code scanner stands: **the runs tile the line
//! exactly.** Every path emits whole slices left to right, and a character no
//! rule wants falls through to the same one-character plain run the code
//! scanner uses.
//!
//! Markdown's one construct that crosses a line break is the fenced code
//! block, and `State::Fence` carries it: the fence character and the language
//! the tag resolved to. Lines inside a fence with a known language are coloured
//! by the code scanner itself, so ```` ```rust ```` reads the same here as the
//! same block does in the preview. The fence's inner scan restarts on every
//! line - a block comment inside a fenced block loses its colour on its second
//! line - which is the price of keeping the state flat and `Copy`.

use super::markup::{Open, Token, char_len, emit, push, MAX_HIGHLIGHT_LINE};
use super::{Lang, State};

/// The runs of one Markdown line, given the state it starts in, and the state
/// the next line starts in.
pub(crate) fn tokenize(line: &str, state: State) -> (Vec<(String, Token)>, State) {
    // The same guard as the code scanner: a line this long is drawn plain
    // rather than a quarter of a second behind every frame.
    if line.len() > MAX_HIGHLIGHT_LINE {
        let runs = if line.is_empty() {
            Vec::new()
        } else {
            vec![(line.to_owned(), Token::Plain)]
        };
        return (runs, state);
    }
    let mut runs: Vec<(String, Token)> = Vec::new();
    let mut open = Open::idle();
    let out = scan(line, state, true, &mut |a, b, token| {
        emit(&mut runs, &mut open, line, a, b, token, false)
    });
    push(&mut runs, &mut open, line);
    (runs, out)
}

/// The state the next Markdown line starts in, without building any runs.
///
/// The same scan as [`tokenize`] with the colouring thrown away, so the two can
/// never disagree about where a fence ends.
pub(crate) fn advance(line: &str, state: State) -> State {
    if line.len() > MAX_HIGHLIGHT_LINE {
        return state;
    }
    scan(line, state, false, &mut |_, _, _| {})
}

/// The one scan behind both. Reports each run as `(start, end, token)` to
/// `sink`, left to right, and returns the state the line leaves behind.
///
/// `colour` is false for [`advance`], which wants only the state: inside a
/// fence, the delegated code scan would otherwise build every run just to drop
/// them, once per line, in the background worker that maps the file.
fn scan(
    line: &str,
    state: State,
    colour: bool,
    sink: &mut dyn FnMut(usize, usize, Token),
) -> State {
    let bytes = line.as_bytes();

    // A line inside a fence first has to be told apart from its fence's close.
    if let State::Fence { ch, lang } = state {
        if fence_close(line, ch) {
            sink(0, bytes.len(), Token::Fence);
            return State::Normal;
        }
        match lang {
            Some(code) if colour => {
                // The block is code: colour it as code. The delegated scan
                // starts fresh on every line, and the state it leaves is
                // dropped, so nothing inside a fence can leak past the fence.
                let (runs, _) = super::tokenize_with(line, &code, State::Normal);
                let mut at = 0usize;
                for (run, token) in runs {
                    let end = at + run.len();
                    sink(at, end, token);
                    at = end;
                }
            }
            None if colour => sink(0, bytes.len(), Token::Plain),
            _ => {}
        }
        return State::Fence { ch, lang };
    }

    // Up to three spaces of indent are allowed in front of every block-level
    // marker; four turn the line into code, which is left uncoloured here.
    let indent = indent_of(line);
    let content = &line[indent..];

    // A fence opens on this line: the whole line is fence, tag and all.
    if let Some((ch, lang)) = fence_open(content) {
        sink(0, bytes.len(), Token::Fence);
        return State::Fence { ch, lang };
    }

    // An ATX heading: one to six `#` and then a space, a tab, or nothing. A
    // run of seven or more is plain text; so is `#tag` with no space.
    let hashes = content.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes)
        && content
            .as_bytes()
            .get(hashes)
            .is_none_or(|&b| b == b' ' || b == b'\t')
    {
        sink(0, bytes.len(), Token::Heading);
        return State::Normal;
    }

    // A blockquote: one or more `>` markers, a single space allowed after
    // each, and then the quoted text, which is coloured as ordinary prose.
    if content.starts_with('>') {
        let mut i = indent;
        while i < bytes.len() {
            match bytes[i] {
                b'>' => i += 1,
                b' ' if bytes.get(i + 1) == Some(&b'>') => i += 1,
                b'\t' if bytes.get(i + 1) == Some(&b'>') => i += 1,
                _ => break,
            }
        }
        // The one space between the last marker and the text belongs to the
        // markers, so the quote reads as one glyph and a gap.
        if bytes.get(i) == Some(&b' ') {
            i += 1;
        }
        // The indent before the markers is plain, because `emit` merges it
        // away only when it is followed by more plain text.
        if indent > 0 {
            sink(0, indent, Token::Plain);
        }
        sink(indent, i, Token::Marker);
        inline(line, i, sink);
        return State::Normal;
    }

    // A thematic break, before list markers: a run of three or more of one of
    // `-`, `*`, `_` with nothing else on the line.
    if is_thematic_break(content) {
        sink(0, bytes.len(), Token::Marker);
        return State::Normal;
    }

    // A list marker: `- `, `* `, `+ `, or `1. ` / `1) `. A task box straight
    // after folds into the same run, so the checkbox reads as part of the
    // marker rather than as broken prose.
    if let Some(marker) = list_marker(content) {
        let mut end = indent + marker;
        let rest = &line[end..];
        for task in ["[ ] ", "[x] ", "[X] "] {
            if rest.starts_with(task) {
                end += task.len();
                break;
            }
        }
        if indent > 0 {
            sink(0, indent, Token::Plain);
        }
        sink(indent, end, Token::Marker);
        inline(line, end, sink);
        return State::Normal;
    }

    if indent > 0 {
        sink(0, indent, Token::Plain);
    }
    inline(line, indent, sink);
    State::Normal
}

/// Inline Markdown: code spans, emphasis, strong, strike, links, autolinks.
///
/// A marker that never closes colours to the end of the line, the way an
/// unterminated string does in code - the normal state of a line halfway
/// through typing, and the only way to keep the runs tiling without lookahead.
fn inline(line: &str, from: usize, sink: &mut dyn FnMut(usize, usize, Token)) {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            // An escaped punctuation character is literal, both halves of it.
            b'\\' if bytes
                .get(i + 1)
                .is_some_and(|&n| n.is_ascii_punctuation()) =>
            {
                sink(i, i + 2, Token::Plain);
                i += 2;
            }
            b'`' => {
                let n = run_of(bytes, i, b'`');
                match find_run(line, i + n, b'`', n) {
                    Some(end) => {
                        sink(i, end, Token::CodeSpan);
                        i = end;
                    }
                    None => {
                        sink(i, bytes.len(), Token::CodeSpan);
                        i = bytes.len();
                    }
                }
            }
            b'*' | b'_' => {
                let n = run_of(bytes, i, b);
                // Two or more: try strong first, and let `***x***` read as
                // strong over the whole thing rather than two nested spans.
                if n >= 2 {
                    let marker = &line[i..i + 2];
                    if let Some(j) = line[i + 2..].find(marker) {
                        let end = i + 2 + j + 2;
                        sink(i, end, Token::Strong);
                        i = end;
                        continue;
                    }
                }
                // One marker: emphasis, with `_` held back to the flanking
                // rule so snake_case and file_names never colour.
                if b == b'*' || (flanks_open(line, i) && n == 1) {
                    let one = &line[i..i + 1];
                    if let Some(j) = line[i + 1..].find(one) {
                        let end = i + 1 + j + 1;
                        sink(i, end, Token::Emph);
                        i = end;
                        continue;
                    }
                }
                sink(i, i + n, Token::Plain);
                i += n;
            }
            b'~' => {
                let n = run_of(bytes, i, b'~');
                if n >= 2
                    && let Some(j) = line[i + 2..].find("~~")
                {
                    let end = i + 2 + j + 2;
                    sink(i, end, Token::Emph);
                    i = end;
                    continue;
                }
                sink(i, i + n, Token::Plain);
                i += n;
            }
            b'[' => match link_end(line, i) {
                Some(end) => {
                    sink(i, end, Token::Link);
                    i = end;
                }
                None => {
                    sink(i, i + 1, Token::Plain);
                    i += 1;
                }
            },
            b'!' if bytes.get(i + 1) == Some(&b'[') => match link_end(line, i + 1) {
                Some(end) => {
                    sink(i, end, Token::Link);
                    i = end;
                }
                None => {
                    sink(i, i + 1, Token::Plain);
                    i += 1;
                }
            },
            // An autolink: `<url>` or `<mail@host>`, no spaces inside.
            b'<' => {
                let close = line[i + 1..].find('>').map(|j| i + 1 + j);
                let inner = close.map(|j| &line[i + 1..j]).unwrap_or("");
                if let Some(end) = close
                    && !inner.contains(' ')
                    && (inner.starts_with("http://")
                        || inner.starts_with("https://")
                        || (inner.contains('@') && !inner.starts_with('<')))
                {
                    sink(i, end + 1, Token::Link);
                    i = end + 1;
                } else {
                    sink(i, i + 1, Token::Plain);
                    i += 1;
                }
            }
            _ => {
                let end = i + char_len(line, i);
                sink(i, end, Token::Plain);
                i = end;
            }
        }
    }
}

/// Where the `[text](url)` that starts at `[` (or `![` for images) ends, or
/// `None` when the brackets do not make a link on this line.
fn link_end(line: &str, bracket: usize) -> Option<usize> {
    let mid = line[bracket + 1..].find("](")? + bracket + 1;
    // The URL may hold balanced parentheses, the way Wikipedia URLs do; a
    // closing `)` at depth zero is the end.
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    for (k, &b) in bytes.iter().enumerate().skip(mid + 2) {
        match b {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    return Some(k + 1);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Whether the `_` at `i` can open emphasis: nothing alphanumeric just before
/// it, and no whitespace just after. Word-internal underscores stay plain.
fn flanks_open(line: &str, i: usize) -> bool {
    let bytes = line.as_bytes();
    let before_ok = i == 0
        || !bytes[i - 1].is_ascii_alphanumeric()
        && bytes[i - 1] != b'_';
    let after_ok = bytes
        .get(i + 1)
        .is_some_and(|&b| b != b' ' && b != b'\t');
    before_ok && after_ok
}

/// How many of `ch` sit in a row at `i`.
fn run_of(bytes: &[u8], i: usize, ch: u8) -> usize {
    bytes[i..].iter().take_while(|&&b| b == ch).count()
}

/// The end of the first run of at least `n` of `ch` at or after `from`, as an
/// offset just past it.
fn find_run(line: &str, from: usize, ch: u8, n: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == ch {
            let run = run_of(bytes, i, ch);
            if run >= n {
                return Some(i + run);
            }
            i += run;
        } else {
            i += char_len(line, i);
        }
    }
    None
}

/// How many spaces and tabs open the line.
fn indent_of(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count()
}

/// Whether the fence that opens on `content` is recognisable: at least three
/// of `` ` `` or `~`, with an info string that a backtick fence may not repeat.
///
/// Returns the fence character and the language the tag names, `None` for a
/// tag this editor has no tokenizer for.
fn fence_open(content: &str) -> Option<(u8, Option<Lang>)> {
    let bytes = content.as_bytes();
    let ch = *bytes.first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let n = run_of(bytes, 0, ch);
    if n < 3 {
        return None;
    }
    let info = &content[n..];
    if ch == b'`' && info.contains('`') {
        return None;
    }
    // The first word of the info string is the language; the rest are
    // attributes, which nobody colours.
    let tag = info.split_whitespace().next().unwrap_or("");
    let lang = crate::editing::fence_lang(tag);
    Some((ch, lang))
}

/// Whether `line` closes a fence of `ch`: three or more of the character and
/// nothing else on the line.
fn fence_close(line: &str, ch: u8) -> bool {
    let bytes = &line.as_bytes()[indent_of(line).min(3)..];
    let n = run_of(bytes, 0, ch);
    n >= 3 && bytes[n..].iter().all(|&b| b == b' ' || b == b'\t')
}

/// Whether `content` is a thematic break: three or more of one of `-`, `*`,
/// `_`, with only spaces and tabs around them.
fn is_thematic_break(content: &str) -> bool {
    let bytes = content.as_bytes();
    let ch = match bytes.first() {
        Some(&b) if matches!(b, b'-' | b'*' | b'_') => b,
        _ => return false,
    };
    let n = run_of(bytes, 0, ch);
    n >= 3 && bytes[n..].iter().all(|&b| b == b' ' || b == b'\t')
}

/// Length of the list marker at the start of `content`, or `None`: `- `, `* `,
/// `+ `, or up to nine digits with `.` or `)` and a space.
fn list_marker(content: &str) -> Option<usize> {
    let bytes = content.as_bytes();
    match bytes.first()? {
        b'-' | b'*' | b'+' => match bytes.get(1) {
            Some(b' ') | Some(b'\t') => Some(2),
            _ => None,
        },
        b'0'..=b'9' => {
            let digits = bytes
                .iter()
                .take_while(|&&b| b.is_ascii_digit())
                .count();
            if digits > 9 {
                return None;
            }
            match bytes.get(digits) {
                Some(b'.') | Some(b')') => match bytes.get(digits + 1) {
                    Some(b' ') | Some(b'\t') => Some(digits + 2),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runs laid end to end, which is what the caller does to draw them.
    fn joined(runs: &[(String, Token)]) -> String {
        runs.iter().map(|(r, _)| r.as_str()).collect()
    }

    /// Tokenizes a line from `State::Normal` and checks the invariant that
    /// outranks the colours: the runs are the line, back to back.
    fn check(line: &str) -> Vec<(String, Token)> {
        let (runs, _) = tokenize(line, State::Normal);
        assert_eq!(joined(&runs), line, "runs do not reproduce {line:?}");
        for (run, _) in &runs {
            assert!(!run.is_empty(), "empty run in {runs:?}");
        }
        runs
    }

    #[test]
    fn a_heading_colours_whole_line() {
        let runs = check("## Install");
        assert_eq!(runs, vec![("## Install".to_owned(), Token::Heading)]);
        // Six is the limit, and `#######` is not a heading.
        assert_eq!(check("###### six")[0].1, Token::Heading);
        assert_eq!(check("####### seven")[0].1, Token::Plain);
        // A hash with no space after it is a tag, not a heading.
        assert_eq!(check("#tag")[0].1, Token::Plain);
        // A bare marker on its own line is still a heading.
        assert_eq!(check("##")[0].1, Token::Heading);
    }

    #[test]
    fn indented_headings_are_still_headings() {
        for line in ["  # two spaces", "   ### three", "\t# tab"] {
            assert_eq!(check(line)[0].1, Token::Heading, "{line:?}");
        }
    }

    #[test]
    fn a_list_marker_colours_and_the_text_stays_plain() {
        let runs = check("- an item");
        assert_eq!(
            runs,
            vec![("- ".to_owned(), Token::Marker), ("an item".to_owned(), Token::Plain)]
        );
        let runs = check("12. twelfth");
        assert_eq!(runs[0], ("12. ".to_owned(), Token::Marker));
        assert_eq!(check("+ plus")[0].1, Token::Marker);
        assert_eq!(check("* star")[0].1, Token::Marker);
        // No space after the marker: emphasis and text, not a list.
        assert_eq!(check("-5 degrees")[0].1, Token::Plain);
    }

    #[test]
    fn a_task_box_is_part_of_the_marker() {
        let runs = check("- [ ] milk");
        assert_eq!(
            runs[0],
            ("- [ ] ".to_owned(), Token::Marker),
            "the checkbox belongs to the marker run"
        );
        assert_eq!(runs[1], ("milk".to_owned(), Token::Plain));
        let runs = check("- [x] done");
        assert_eq!(runs[0], ("- [x] ".to_owned(), Token::Marker));
    }

    #[test]
    fn a_blockquote_colours_its_markers_and_keeps_the_text() {
        let runs = check("> quoted text");
        assert_eq!(
            runs,
            vec![("> ".to_owned(), Token::Marker), ("quoted text".to_owned(), Token::Plain)]
        );
        // Nested quotes keep every marker in one run.
        let runs = check(">> deep");
        assert_eq!(runs[0], (">> ".to_owned(), Token::Marker));
    }

    #[test]
    fn a_thematic_break_is_a_marker() {
        for line in ["---", "***", "___", "   ---", "- - -"] {
            // `- - -` is a list whose text starts with `-`, which is fine.
            let want = if line == "- - -" { Token::Marker } else { Token::Marker };
            assert_eq!(check(line)[0].1, want, "{line:?}");
        }
        assert_eq!(check("--")[0].1, Token::Plain, "two is not a break");
    }

    #[test]
    fn a_fence_open_line_is_all_fence() {
        let (runs, next) = tokenize("```rust", State::Normal);
        assert_eq!(runs, vec![("```rust".to_owned(), Token::Fence)]);
        assert_eq!(
            next,
            State::Fence {
                ch: b'`',
                lang: crate::editing::fence_lang("rust")
            }
        );
        // A tilde fence, and a tag with attributes after it.
        let (runs, next) = tokenize("~~~ ts ignore", State::Normal);
        assert_eq!(runs, vec![("~~~ ts ignore".to_owned(), Token::Fence)]);
        assert!(matches!(next, State::Fence { ch: b'~', lang: Some(_) }));
        // Two backticks are not a fence, but they do open an unterminated
        // code span, which colours to the end of the line like any marker
        // that never closes.
        let (runs, _) = tokenize("`` code", State::Normal);
        assert_eq!(runs[0].1, Token::CodeSpan);
    }

    #[test]
    fn a_fence_closes_and_the_state_returns_to_normal() {
        let (runs, next) = tokenize("```", State::Fence {
            ch: b'`',
            lang: None,
        });
        assert_eq!(runs, vec![("```".to_owned(), Token::Fence)]);
        assert_eq!(next, State::Normal);
    }

    #[test]
    fn a_rust_fence_delegates_to_the_code_tokenizer() {
        let state = State::Fence {
            ch: b'`',
            lang: crate::editing::fence_lang("rust"),
        };
        let (runs, next) = tokenize("let x = 1;", state);
        assert_eq!(
            runs.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            vec![Token::Keyword, Token::Plain, Token::Number, Token::Plain],
            "the fence's language colours the line"
        );
        assert_eq!(joined(&runs), "let x = 1;");
        assert_eq!(next, state, "the fence stays open");
    }

    #[test]
    fn an_unknown_fence_stays_plain() {
        let state = State::Fence { ch: b'`', lang: None };
        let (runs, next) = tokenize("anything at all", state);
        assert_eq!(runs, vec![("anything at all".to_owned(), Token::Plain)]);
        assert_eq!(next, state);
    }

    #[test]
    fn emphasis_colours_both_markers() {
        let (runs, _) = tokenize("a *word* b", State::Normal);
        assert_eq!(joined(&runs), "a *word* b");
        // Collapsed: `a ` is one plain run, the span, then ` b`.
        let mut classes: Vec<Token> = Vec::new();
        for (_, t) in &runs {
            if classes.last() != Some(t) {
                classes.push(*t);
            }
        }
        assert_eq!(classes, vec![Token::Plain, Token::Emph, Token::Plain]);
        let runs = check("**bold**");
        assert_eq!(runs, vec![("**bold**".to_owned(), Token::Strong)]);
    }

    #[test]
    fn an_unterminated_emphasis_stays_plain_but_a_backtick_does_not() {
        // A stray `*` is far more often a multiplication sign or a footnote
        // star in prose than half a span, so with no closer it stays plain -
        // the opposite trade to the backtick, where half a span is nearly
        // always a span being typed.
        let (runs, _) = tokenize("a *b c", State::Normal);
        assert_eq!(joined(&runs), "a *b c");
        assert!(!runs.iter().any(|(_, t)| *t == Token::Emph), "{runs:?}");
        let (runs, _) = tokenize("a **b c", State::Normal);
        assert!(!runs.iter().any(|(_, t)| *t == Token::Strong), "{runs:?}");
        let (runs, _) = tokenize("3 * 4 = 12", State::Normal);
        assert!(
            runs.iter().all(|(_, t)| *t == Token::Plain),
            "arithmetic read as emphasis: {runs:?}"
        );
        // A backtick with no close is a span being typed, so it colours on.
        let (runs, _) = tokenize("a `b c", State::Normal);
        assert_eq!(runs.last().map(|(_, t)| *t), Some(Token::CodeSpan));
    }

    #[test]
    fn snake_case_and_file_names_are_not_emphasis() {
        // A word-internal `_` has a letter on its left, so it cannot open.
        let (runs, _) = tokenize("some_var_name here", State::Normal);
        assert!(
            !runs.iter().any(|(_, t)| *t == Token::Emph),
            "snake_case was taken for emphasis: {runs:?}"
        );
        assert_eq!(joined(&runs), "some_var_name here");
    }

    #[test]
    fn strike_and_code_span_round_trip() {
        let runs = check("~~gone~~");
        assert_eq!(runs, vec![("~~gone~~".to_owned(), Token::Emph)]);
        let runs = check("use `x = 1` here");
        assert_eq!(
            runs.iter().map(|(_, t)| *t).collect::<Vec<_>>().len(),
            runs.len()
        );
        assert!(
            runs.iter()
                .any(|(r, t)| r == "`x = 1`" && *t == Token::CodeSpan),
            "{runs:?}"
        );
    }

    #[test]
    fn links_and_autolinks_round_trip() {
        let runs = check("see [the docs](https://x.y) now");
        assert_eq!(joined(&runs), "see [the docs](https://x.y) now");
        assert!(
            runs.iter()
                .any(|(r, t)| r == "[the docs](https://x.y)" && *t == Token::Link),
            "{runs:?}"
        );
        // An image, whose `!` belongs to the link run.
        let runs = check("![alt](img.png)");
        assert_eq!(runs, vec![("![alt](img.png)".to_owned(), Token::Link)]);
        // A bare `[` with no `](` is one plain character.
        let runs = check("array[0]");
        assert_eq!(joined(&runs), "array[0]");
        assert!(!runs.iter().any(|(_, t)| *t == Token::Link));
        // An autolink.
        let runs = check("mail <a@b.c> end");
        assert!(runs.iter().any(|(r, t)| r == "<a@b.c>" && *t == Token::Link));
    }

    #[test]
    fn an_escape_is_literal_and_its_backslash_survives() {
        let runs = check(r"a \* b");
        assert_eq!(joined(&runs), r"a \* b");
        assert!(!runs.iter().any(|(_, t)| *t == Token::Emph), "{runs:?}");
    }

    #[test]
    fn runs_tile_the_line_for_awkward_input() {
        // The property the code scanner's tests hold it to, held here too.
        const LINES: &[&str] = &[
            "",
            " ",
            "   ",
            "\t\t",
            "#",
            "# ",
            "######",
            "#######",
            "#tag",
            ">",
            "> > ",
            ">>",
            "-",
            "- ",
            "* * *",
            "___",
            "----",
            "`",
            "``",
            "```",
            "``````",
            "```rust",
            "~~~",
            "**",
            "****",
            "***x***",
            "*x_y*",
            "_ _",
            "[",
            "[]",
            "[]()",
            "[a](b(c)d)",
            "![]()",
            "<",
            "<not a link",
            "<https://x.y>",
            "\\",
            "\\*",
            "🙂 # heading 🙂",
            "1. one\n",
        ];
        for &line in LINES {
            let (runs, _) = tokenize(line, State::Normal);
            assert_eq!(joined(&runs), line, "runs do not reproduce {line:?}");
            for (run, _) in &runs {
                assert!(!run.is_empty(), "empty run in {runs:?} for {line:?}");
            }
        }
    }

    #[test]
    fn advance_agrees_with_tokenize_about_fences() {
        // The state cache calls `advance`; the painter calls `tokenize`. The
        // two must walk the same state machine.
        let lines = ["```rust", "let x = 1;", "```", "plain again"];
        let mut state = State::Normal;
        for line in lines {
            let (_, after_tokenize) = tokenize(line, state);
            let after_advance = advance(line, state);
            assert_eq!(after_tokenize, after_advance, "for {line:?}");
            state = after_advance;
        }
        assert_eq!(state, State::Normal);
    }

    #[test]
    fn a_very_long_line_is_drawn_plain_and_keeps_its_state() {
        let line = "x".repeat(MAX_HIGHLIGHT_LINE + 1);
        let (runs, next) = tokenize(&line, State::Normal);
        assert_eq!(runs, vec![(line.clone(), Token::Plain)]);
        assert_eq!(next, State::Normal);
    }
}
