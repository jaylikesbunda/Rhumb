//! Pure text transforms that make the built-in editor feel like a code editor:
//! indenting, auto-indent, bracket completion and comment toggling.
//!
//! Everything here works on `String` plus *character* indices (what egui's
//! text cursor uses), and is unit tested.

/// The indent unit used everywhere.
pub const INDENT: &str = "    ";

/// Character index to byte offset, clamped to the string length.
pub fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

/// Byte offset to character index.
pub fn byte_to_char(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx.min(s.len())].chars().count()
}

/// The character at `char_idx`, if any.
pub fn char_at(s: &str, char_idx: usize) -> Option<char> {
    s.chars().nth(char_idx)
}

/// Byte range covering the full lines touched by `[from, to)` (char indices).
pub fn line_range(s: &str, from: usize, to: usize) -> std::ops::Range<usize> {
    let start_byte = char_to_byte(s, from);
    let line_start = s[..start_byte].rfind('\n').map_or(0, |i| i + 1);
    let end_byte = char_to_byte(s, to);
    let line_end = s[end_byte..].find('\n').map_or(s.len(), |i| end_byte + i);
    line_start..line_end
}

/// Leading whitespace of a line.
fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Indents or outdents every line in `range`. Returns the character index the
/// cursor should move to, or `None` to leave it alone.
pub fn indent(s: &mut String, range: std::ops::Range<usize>, outdent: bool) -> Option<usize> {
    let lines = line_range(s, range.start, range.end);
    let slice = &s[lines.clone()];
    let mut out = String::with_capacity(slice.len() + 16);
    let mut removed = 0usize;

    for (i, line) in slice.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if outdent {
            let removable = leading_ws(line).len().min(INDENT.len());
            if removable > 0 && !line.trim().is_empty() {
                out.push_str(&line[removable..]);
                removed += line[..removable].chars().count();
            } else {
                out.push_str(line);
            }
        } else {
            out.push_str(INDENT);
            out.push_str(line);
        }
    }

    let whole_selection = range.end > range.start;
    s.replace_range(lines.clone(), &out);

    // Indenting leaves the cursor after the new spaces; outdenting has to pull
    // it back by however many characters were removed before it.
    if outdent && removed > 0 {
        let start_char = byte_to_char(s, lines.start);
        let new_start = (start_char as isize - removed as isize).max(0) as usize;
        if whole_selection {
            let end_char = byte_to_char(s, lines.end);
            Some(new_start + end_char.saturating_sub(start_char))
        } else {
            Some(new_start)
        }
    } else {
        None
    }
}

/// After pressing Enter, carry the current line's indentation over.
///
/// If the line ends with an opening bracket, add one more level. Returns the
/// new cursor position, in character indices.
pub fn auto_indent(s: &mut String, cursor: usize) -> usize {
    let byte = char_to_byte(s, cursor);
    let line_start = s[..byte].rfind('\n').map_or(0, |i| i + 1);
    let line = &s[line_start..byte];
    let ws = leading_ws(line);
    let extra = if line.trim_end().ends_with(['{', '[', '(']) {
        INDENT
    } else {
        ""
    };
    if ws.is_empty() && extra.is_empty() {
        return cursor;
    }
    let insert = format!("{ws}{extra}");
    s.insert_str(byte, &insert);
    cursor + insert.chars().count()
}

/// Opening bracket to the pair inserted after it.
pub const PAIRS: &[(char, char)] = &[
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('"', '"'),
    ('\'', '\''),
    ('`', '`'),
];

/// Is this one of the characters we auto-close?
pub fn is_opener(ch: char) -> bool {
    PAIRS.iter().any(|(open, _)| *open == ch)
}

/// Inserts the closing half of a pair when the user types an opener.
///
/// `cursor` is the character index *after* the typed opener. Returns the new
/// cursor position (character index), or `None` when nothing was inserted.
pub fn auto_close(s: &mut String, cursor: usize) -> Option<usize> {
    let byte = char_to_byte(s, cursor);
    let prev = s[..byte].chars().next_back()?;
    if !is_opener(prev) {
        return None;
    }
    let &(_, closer) = PAIRS.iter().find(|(open, _)| *open == prev)?;
    // Do not close when the user is typing over an existing closer.
    if let Some(next) = s[byte..].chars().next()
        && next == closer
    {
        return None;
    }
    s.insert(byte, closer);
    // The caret stays where the user typed: between the two halves.
    Some(cursor)
}

/// True when the caret sits directly after an opener whose closer is the very
/// next character, so pressing that closer again should step past it.
pub fn should_skip_closer(s: &str, cursor: usize) -> bool {
    if cursor == 0 {
        return false;
    }
    let Some(before) = char_at(s, cursor - 1) else {
        return false;
    };
    let Some(after) = char_at(s, cursor) else {
        return false;
    };
    PAIRS
        .iter()
        .any(|(open, close)| *open == before && *close == after)
}

/// Toggles `comment` on every line in the selection.
///
/// Returns the new selection bounds as character indices.
pub fn toggle_comment(
    s: &mut String,
    range: std::ops::Range<usize>,
    comment: &str,
) -> Option<usize> {
    let lines = line_range(s, range.start, range.end);
    let slice = s[lines.clone()].to_owned();
    let rows: Vec<&str> = slice.split('\n').collect();
    let all_commented = !rows.is_empty()
        && rows
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| l.trim_start().starts_with(comment));

    let mut out = String::with_capacity(slice.len() + 16);
    for (i, line) in rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim().is_empty() {
            out.push_str(line);
            continue;
        }
        if all_commented {
            let ws_len = line.len() - line.trim_start().len();
            let after = &line[ws_len..];
            let stripped = after.strip_prefix(comment).unwrap_or(after);
            // Drop one leading space too, if the comment added it.
            let stripped = stripped.strip_prefix(' ').unwrap_or(stripped);
            out.push_str(&line[..ws_len]);
            out.push_str(stripped);
        } else {
            let ws_len = line.len() - line.trim_start().len();
            out.push_str(&line[..ws_len]);
            out.push_str(comment);
            out.push(' ');
            out.push_str(&line[ws_len..]);
        }
    }
    let start_char = byte_to_char(s, lines.start);
    let end_char = byte_to_char(s, lines.end);
    s.replace_range(lines.clone(), &out);
    let _ = end_char;
    Some(start_char)
}

/// The comment token for a file, if the language has one.
pub fn comment_token(path: &std::path::Path) -> &'static str {
    match crate::fs_model::ext_of(path).as_str() {
        "rs" | "toml" | "cfg" | "conf" | "ini" | "py" | "rb" | "pl" | "env" => "#",
        "sh" | "bash" | "zsh" | "fish" | "yml" | "yaml" | "makefile" | "dockerfile"
        | "gitignore" => "#",
        "sql" | "hs" | "lua" => "--",
        _ => "//",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_byte_round_trip() {
        let s = "héllo wörld";
        for (ci, ch) in s.chars().enumerate() {
            let b = char_to_byte(s, ci);
            assert_eq!(byte_to_char(s, b), ci);
            assert!(s[b..].starts_with(ch));
        }
    }

    #[test]
    fn indent_adds_one_level() {
        let mut s = String::from("fn main() {\nprintln!();\n}");
        let end = s.chars().count();
        indent(&mut s, 0..end, false);
        assert_eq!(s, "    fn main() {\n    println!();\n    }");
    }

    #[test]
    fn outdent_removes_one_level() {
        let mut s = String::from("    a\n        b");
        let end = s.chars().count();
        indent(&mut s, 0..end, true);
        assert_eq!(s, "a\n    b");
    }

    #[test]
    fn outdent_leaves_short_lines_alone() {
        let mut s = String::from("  a\nb");
        let end = s.chars().count();
        indent(&mut s, 0..end, true);
        assert_eq!(s, "a\nb");
    }

    #[test]
    fn indent_only_touches_selected_lines() {
        let mut s = String::from("a\nb\nc");
        indent(&mut s, 2..3, false); // inside "b"
        assert_eq!(s, "a\n    b\nc");
    }

    #[test]
    fn auto_indent_carries_whitespace() {
        let mut s = String::from("    let x = 1;");
        let end = s.chars().count();
        let caret = auto_indent(&mut s, end);
        assert_eq!(s, "    let x = 1;    ");
        assert_eq!(caret, s.chars().count());
    }

    #[test]
    fn auto_indent_adds_level_after_brace() {
        let mut s = String::from("  fn f() {");
        let end = s.chars().count();
        auto_indent(&mut s, end);
        assert_eq!(s, "  fn f() {      ");
    }

    #[test]
    fn auto_close_inserts_matching_bracket() {
        let mut s = String::from("foo(");
        let c = auto_close(&mut s, 4).expect("closed");
        assert_eq!(s, "foo()");
        assert_eq!(c, 4, "caret sits between the pair");
    }

    #[test]
    fn auto_close_does_not_duplicate_existing_closer() {
        let mut s = String::from("foo()");
        assert!(auto_close(&mut s, 4).is_none());
        assert_eq!(s, "foo()");
    }

    #[test]
    fn skip_over_typed_closer() {
        // The state after `auto_close` ran: the caret sits inside a pair.
        assert!(should_skip_closer("()", 1));
        assert!(!should_skip_closer("(x)", 1));
        assert!(!should_skip_closer("()", 0));
    }

    #[test]
    fn should_skip_matches_auto_close_output() {
        let mut s = String::from("foo(");
        let caret = auto_close(&mut s, 4).expect("closed");
        assert_eq!(s, "foo()");
        assert!(should_skip_closer(&s, caret));
    }

    #[test]
    fn line_range_spans_whole_lines() {
        let s = "one\ntwo\nthree";
        // The line holding characters 5..6 is "two".
        assert_eq!(line_range(s, 5, 6), 4..7);
        assert_eq!(line_range(s, 0, 0), 0..3);
    }

    #[test]
    fn toggle_comment_adds_then_removes() {
        let mut s = String::from("alpha\nbeta");
        let all = s.chars().count();
        toggle_comment(&mut s, 0..all, "//");
        assert_eq!(s, "// alpha\n// beta");
        let all = s.chars().count();
        toggle_comment(&mut s, 0..all, "//");
        assert_eq!(s, "alpha\nbeta");
    }

    #[test]
    fn toggle_comment_on_one_line_only() {
        let mut s = String::from("alpha\nbeta");
        toggle_comment(&mut s, 0..5, "//");
        assert_eq!(s, "// alpha\nbeta");
    }

    #[test]
    fn toggle_comment_uncomments_block() {
        let mut s = String::from("// a\n// b");
        let all = s.chars().count();
        toggle_comment(&mut s, 0..all, "//");
        assert_eq!(s, "a\nb");
    }

    #[test]
    fn comment_tokens() {
        assert_eq!(comment_token(std::path::Path::new("a.rs")), "#");
        assert_eq!(comment_token(std::path::Path::new("a.rs2")), "//");
        assert_eq!(comment_token(std::path::Path::new("a.py")), "#");
        assert_eq!(comment_token(std::path::Path::new("a.md")), "//");
    }
}
