//! Pure text transforms that make the built-in editor feel like a code editor:
//! indenting, auto-indent, bracket completion and comment toggling.
//!
//! Everything here works on `String` plus *character* indices (what egui's
//! text cursor uses), and is unit tested.

/// The indent unit used everywhere.
pub const INDENT: &str = "    ";

use crate::buffer::Buffer;

/// The character range covering the full lines touched by `[from, to)`, without
/// the newline that ends the last of them.
pub fn line_span(s: &Buffer, from: usize, to: usize) -> std::ops::Range<usize> {
    s.line_start(s.line_of_char(from))..s.line_end(s.line_of_char(to))
}

/// Leading whitespace of a line.
fn leading_ws(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// Indents or outdents every line in `range`. Returns the character index the
/// cursor should move to, or `None` to leave it alone.
pub fn indent(s: &mut Buffer, range: std::ops::Range<usize>, outdent: bool) -> Option<usize> {
    let lines = line_span(s, range.start, range.end);
    let slice = s.slice(lines.start, lines.end).into_owned();
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
    let old_chars = lines.end - lines.start;
    s.replace(lines.start, lines.end, &out);

    // Indenting leaves the cursor after the new spaces; outdenting has to pull
    // it back by however many characters were removed before it.
    if outdent && removed > 0 {
        let new_start = (lines.start as isize - removed as isize).max(0) as usize;
        if whole_selection {
            Some(new_start + old_chars)
        } else {
            Some(new_start)
        }
    } else {
        None
    }
}

/// The indentation a new line should begin with, worked out from the line
/// being split at `cursor`.
///
/// The current line's own indentation is carried over, plus one more level if
/// that line ends with an opening bracket. This is the part that decides *what*
/// to type, and it is asked before the newline is inserted: afterwards the caret
/// sits on an empty new line, and an empty line has no indentation to copy.
pub fn indent_for_new_line(s: &Buffer, cursor: usize) -> String {
    let cursor = cursor.min(s.len_chars());
    let line_start = s.line_start(s.line_of_char(cursor));
    let line = s.slice(line_start, cursor);
    let mut out = leading_ws(&line).to_owned();
    if line.trim_end().ends_with(['{', '[', '(']) {
        out.push_str(INDENT);
    }
    out
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
pub fn auto_close(s: &mut Buffer, cursor: usize) -> Option<usize> {
    let prev = s.char_at(cursor.checked_sub(1)?)?;
    if !is_opener(prev) {
        return None;
    }
    let &(_, closer) = PAIRS.iter().find(|(open, _)| *open == prev)?;
    // Do not close when the user is typing over an existing closer.
    if s.char_at(cursor) == Some(closer) {
        return None;
    }
    // Only where the closer would not be in the way: at the end of a line, before
    // whitespace, or before something that is itself a closer or a separator.
    // A bracket typed in front of a word is wrapping nothing, and a closer dropped
    // between it and the word is one to delete.
    if let Some(next) = s.char_at(cursor)
        && !(next.is_whitespace() || ")]}>,;:.".contains(next))
    {
        return None;
    }
    // A quote that follows a letter or digit is an apostrophe, or the end of a string
    // that was opened somewhere else: never the start of a new pair. `don't` must
    // not come out with a second quote after it.
    if prev == closer {
        let before = cursor.checked_sub(2).and_then(|i| s.char_at(i));
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == prev) {
            return None;
        }
    }
    s.insert(cursor, &closer.to_string());
    // The caret stays where the user typed: between the two halves.
    Some(cursor)
}

/// Whether `c` attaches to the character before it instead of being a letter of its
/// own: a combining mark, a variation selector, an emoji skin tone, or a zero-width
/// joiner. Together with what it attaches to it is one thing on screen, and the
/// caret has no visible place to stand between the two.
pub fn is_extender(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x05BF
        | 0x05C1..=0x05C2
        | 0x05C4..=0x05C5
        | 0x05C7
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200C..=0x200D
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
        | 0x1F3FB..=0x1F3FF
        | 0xE0100..=0xE01EF)
}

/// The boundary after the character at `at`, taking everything that attaches to it
/// along: `a` followed by a combining accent is one step, and so is an emoji joined
/// to another by a zero-width joiner.
pub fn cluster_end(s: &Buffer, at: usize) -> usize {
    let n = s.len_chars();
    if at >= n {
        return n;
    }
    let mut i = at + 1;
    while let Some(c) = s.char_at(i) {
        if !is_extender(c) {
            break;
        }
        i += 1;
        // A joiner glues on the character after it as well.
        if c == '\u{200D}' && i < n {
            i += 1;
        }
    }
    i
}

/// The boundary before the character just before `at`, taking everything attached
/// to it along. The inverse of [`cluster_end`].
pub fn cluster_start(s: &Buffer, at: usize) -> usize {
    let mut i = at.min(s.len_chars());
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 {
        let joined_to_previous = s.char_at(i - 1) == Some('\u{200D}');
        if s.char_at(i).is_some_and(is_extender) || joined_to_previous {
            i -= 1;
        } else {
            break;
        }
    }
    i
}

/// Whether backspace at `cursor` is between a bracket or quote and the closer it
/// brought with it, so that both should go.
pub fn inside_empty_pair(s: &Buffer, cursor: usize) -> bool {
    let (Some(before), Some(after)) = (
        cursor.checked_sub(1).and_then(|i| s.char_at(i)),
        s.char_at(cursor),
    ) else {
        return false;
    };
    PAIRS
        .iter()
        .any(|&(open, close)| open == before && close == after)
}

/// Whether typing `typed` at `cursor` should step over the character already
/// there instead of inserting a second one.
///
/// That is the case when the next character is the same closer and something on
/// this line is still waiting for it: a bracket opener with more openers than
/// closers before the caret, or, for a quote, an odd number of quotes so far, which
/// is to say the caret is inside the string. This covers the closer typed straight
/// after its opener and the one typed after the text between them, which is what
/// `(a)` is. Without the second, typing `)` after the `a` left two.
pub fn types_over_closer(s: &Buffer, cursor: usize, typed: char) -> bool {
    if s.char_at(cursor) != Some(typed) {
        return false;
    }
    let Some(&(open, _)) = PAIRS.iter().find(|(_, close)| *close == typed) else {
        return false;
    };
    let start = s.line_start(s.line_of_char(cursor));
    let before = s.slice(start, cursor);
    if open == typed {
        // A quote opens and closes with the same character: inside a string when an
        // odd number of them have gone by.
        before.chars().filter(|&c| c == typed).count() % 2 == 1
    } else {
        before.chars().filter(|&c| c == open).count()
            > before.chars().filter(|&c| c == typed).count()
    }
}

/// True when the caret sits directly after an opener whose closer is the very
/// next character, so pressing that closer again should step past it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn should_skip_closer(s: &Buffer, cursor: usize) -> bool {
    if cursor == 0 {
        return false;
    }
    let Some(before) = s.char_at(cursor - 1) else {
        return false;
    };
    let Some(after) = s.char_at(cursor) else {
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
    s: &mut Buffer,
    range: std::ops::Range<usize>,
    comment: &str,
) -> Option<usize> {
    let lines = line_span(s, range.start, range.end);
    let slice = s.slice(lines.start, lines.end).into_owned();
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
    s.replace(lines.start, lines.end, &out);
    Some(lines.start)
}

/// Comment token for a file, or `""` when the language has no line comments.
///
/// An empty marker is not a special case to be handled downstream: it means
/// there is nothing to recognise, and no part of the file can be a comment.
pub fn comment_token(path: &std::path::Path) -> &'static str {
    match crate::fs_model::ext_of(path).as_str() {
        // The `#` family, which is most of what anyone writes.
        "rs" | "toml" | "cfg" | "conf" | "ini" | "py" | "pyi" | "rb" | "pl" | "pm" | "env"
        | "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "yml" | "yaml" | "mk" | "makefile"
        | "dockerfile" | "gitignore" | "gitattributes" | "editorconfig" | "r" | "jl" | "nix"
        | "conf.d" | "properties" | "tf" | "tfvars" | "hcl" | "gn" | "gnumakefile" => "#",
        "sql" | "hs" | "lua" | "sql.j2" | "ada" | "lisp" | "el" => "--",
        "ml" | "mli" | "elm" => "-",
        "vim" | "vimrc" => "\"",
        "f90" | "f95" | "for" | "f" | "f03" => "!",
        // Block-comment languages have no line comment at all, so nothing in the
        // file is treated as a comment rather than guessing at `//`.
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "hxx" | "cs" | "js" | "mjs" | "cjs" | "jsx"
        | "ts" | "tsx" | "go" | "java" | "kt" | "kts" | "scala" | "php" | "css" | "scss"
        | "less" | "swift" | "dart" | "zig" | "d" | "proto" | "sol" | "glsl" | "hlsl" => "//",
        "html" | "xml" | "svg" | "vue" | "svelte" | "xsl" | "plist" | "csproj" | "xaml" => "<!--",
        "latex" | "tex" => "%",
        "matlab" | "octave" => "%",
        "json" | "jsonc" | "json5" | "ipynb" | "geojson" => "",
        // Prose, data and everything unrecognised. No comments, because these
        // are not programs, and colouring them as if they were is worse than
        // leaving them plain.
        "md" | "markdown" | "txt" | "log" | "csv" | "tsv" | "rst" | "adoc" | "org" | "pdf"
        | "lock" | "po" | "pot" | "me" => "",
        _ => "",
    }
}

/// What the language of `path` can carry across a line break, for colouring.
///
/// The line marker is the one Ctrl+/ types, except where that is not a comment to
/// the end of the line: HTML's `<!--` is a block, and treating it as a line comment
/// coloured the rest of the line and lost the close.
pub fn lang_for(path: &std::path::Path) -> crate::codeedit::Lang {
    use crate::codeedit::Lang;
    let ext = crate::fs_model::ext_of(path);
    let ext = ext.as_str();
    let c_block = Some(("/*", "*/"));
    match ext {
        "html" | "xml" | "svg" | "vue" | "svelte" | "xsl" | "plist" | "csproj" | "xaml" => Lang {
            line: "",
            block: Some(("<!--", "-->")),
            triple: false,
            template: false,
        },
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => Lang {
            line: "//",
            block: c_block,
            triple: false,
            template: true,
        },
        "py" | "pyi" => Lang {
            line: "#",
            block: None,
            triple: true,
            template: false,
        },
        // TOML has multi-line strings and no block comments.
        "toml" => Lang {
            line: "#",
            block: None,
            triple: true,
            template: false,
        },
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "hxx" | "cs" | "go" | "java" | "kt"
        | "kts" | "scala" | "php" | "css" | "scss" | "less" | "swift" | "dart" | "d" | "proto"
        | "sol" | "glsl" | "hlsl" => Lang {
            line: comment_token(path),
            block: c_block,
            triple: false,
            template: false,
        },
        "sql" => Lang {
            line: "--",
            block: c_block,
            triple: false,
            template: false,
        },
        _ => Lang::line_only(comment_token(path)),
    }
}

/// Whether a file should be syntax coloured at all.
///
/// Only languages this editor actually knows how to recognise. The tokenizer
/// has one keyword list for every language it covers, so running it over a
/// plain text file lights up every ordinary English word that happens to be a
/// keyword in some language — `is`, `in`, `not`, `as`, `use`, `new` — and
/// treats an apostrophe as an unterminated string, so `don't` colours half the
/// line. A file with no language is left alone.
pub fn highlights_code(path: &std::path::Path) -> bool {
    !comment_token(path).is_empty() || {
        // Languages with no line comment but which are unmistakably code.
        matches!(
            crate::fs_model::ext_of(path).as_str(),
            "c" | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "cxx"
                | "hxx"
                | "cs"
                | "js"
                | "mjs"
                | "cjs"
                | "jsx"
                | "ts"
                | "tsx"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "scala"
                | "php"
                | "css"
                | "scss"
                | "less"
                | "swift"
                | "dart"
                | "zig"
                | "d"
                | "proto"
                | "sol"
                | "glsl"
                | "hlsl"
                | "html"
                | "xml"
                | "svg"
                | "vue"
                | "svelte"
                | "json"
                | "jsonc"
                | "json5"
                | "latex"
                | "tex"
                | "matlab"
                | "octave"
                | "vim"
                | "f90"
                | "f95"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    #[test]
    fn indent_adds_one_level() {
        let mut s = Buffer::from("fn main() {\nprintln!();\n}");
        let end = s.len_chars();
        indent(&mut s, 0..end, false);
        assert_eq!(s, "    fn main() {\n    println!();\n    }");
    }

    #[test]
    fn outdent_removes_one_level() {
        let mut s = Buffer::from("    a\n        b");
        let end = s.len_chars();
        indent(&mut s, 0..end, true);
        assert_eq!(s, "a\n    b");
    }

    #[test]
    fn outdent_leaves_short_lines_alone() {
        let mut s = Buffer::from("  a\nb");
        let end = s.len_chars();
        indent(&mut s, 0..end, true);
        assert_eq!(s, "a\nb");
    }

    #[test]
    fn indent_only_touches_selected_lines() {
        let mut s = Buffer::from("a\nb\nc");
        indent(&mut s, 2..3, false); // inside "b"
        assert_eq!(s, "a\n    b\nc");
    }

    #[test]
    fn a_new_line_takes_the_indent_of_the_line_it_splits() {
        // Asked before the newline exists, which is why this is a function of
        // its own rather than something the caller works out afterwards.
        let s = Buffer::from("    let x = 1;");
        assert_eq!(
            indent_for_new_line(&s, 14),
            "    ",
            "whitespace carried over"
        );
        let s = Buffer::from("  fn f() {");
        assert_eq!(
            indent_for_new_line(&s, 10),
            "      ",
            "one level in after a brace"
        );
        let s = Buffer::from("\t\tlet x = 1;");
        assert_eq!(
            indent_for_new_line(&s, 13),
            "\t\t",
            "tabs are kept as they are"
        );
        // Splitting in the middle of a line copies that line's own indent, not
        // the one belonging to whatever follows.
        let s = Buffer::from("    let a = 1;\nno indent here");
        assert_eq!(indent_for_new_line(&s, 10), "    ");
        // A closing bracket steps back out rather than in.
        let s = Buffer::from("      }");
        assert_eq!(indent_for_new_line(&s, 7), "      ");
        // A line with no indentation at all adds none.
        let s = Buffer::from("plain text");
        assert_eq!(indent_for_new_line(&s, 10), "");
    }

    #[test]
    fn auto_close_inserts_matching_bracket() {
        let mut s = Buffer::from("foo(");
        let c = auto_close(&mut s, 4).expect("closed");
        assert_eq!(s, "foo()");
        assert_eq!(c, 4, "caret sits between the pair");
    }

    #[test]
    fn auto_close_does_not_duplicate_existing_closer() {
        let mut s = Buffer::from("foo()");
        assert!(auto_close(&mut s, 4).is_none());
        assert_eq!(s, "foo()");
    }

    #[test]
    fn skip_over_typed_closer() {
        // The state after `auto_close` ran: the caret sits inside a pair.
        assert!(should_skip_closer(&Buffer::from("()"), 1));
        assert!(!should_skip_closer(&Buffer::from("(x)"), 1));
        assert!(!should_skip_closer(&Buffer::from("()"), 0));
    }

    #[test]
    fn should_skip_matches_auto_close_output() {
        let mut s = Buffer::from("foo(");
        let caret = auto_close(&mut s, 4).expect("closed");
        assert_eq!(s, "foo()");
        assert!(should_skip_closer(&s, caret));
    }

    #[test]
    fn line_span_covers_whole_lines() {
        let s = Buffer::from("one\ntwo\nthree");
        // The line holding characters 5..6 is "two".
        assert_eq!(line_span(&s, 5, 6), 4..7);
        assert_eq!(line_span(&s, 0, 0), 0..3);
    }

    #[test]
    fn toggle_comment_adds_then_removes() {
        let mut s = Buffer::from("alpha\nbeta");
        let all = s.len_chars();
        toggle_comment(&mut s, 0..all, "//");
        assert_eq!(s, "// alpha\n// beta");
        let all = s.len_chars();
        toggle_comment(&mut s, 0..all, "//");
        assert_eq!(s, "alpha\nbeta");
    }

    #[test]
    fn toggle_comment_on_one_line_only() {
        let mut s = Buffer::from("alpha\nbeta");
        toggle_comment(&mut s, 0..5, "//");
        assert_eq!(s, "// alpha\nbeta");
    }

    #[test]
    fn toggle_comment_uncomments_block() {
        let mut s = Buffer::from("// a\n// b");
        let all = s.len_chars();
        toggle_comment(&mut s, 0..all, "//");
        assert_eq!(s, "a\nb");
    }

    #[test]
    fn comment_tokens() {
        assert_eq!(comment_token(std::path::Path::new("a.rs")), "#");
        assert_eq!(comment_token(std::path::Path::new("a.py")), "#");
        assert_eq!(comment_token(std::path::Path::new("a.c")), "//");
        assert_eq!(comment_token(std::path::Path::new("a.ts")), "//");
        // An empty marker means the language is not one this editor
        // recognises, or has no line comments at all. Either way nothing in the
        // file is treated as a comment, which is the safe answer for a file whose
        // language is unknown.
        assert_eq!(comment_token(std::path::Path::new("a.md")), "");
        assert_eq!(comment_token(std::path::Path::new("a.txt")), "");
        assert_eq!(comment_token(std::path::Path::new("a.json")), "");
        assert_eq!(comment_token(std::path::Path::new("a.rs2")), "");
        assert_eq!(comment_token(std::path::Path::new("noextension")), "");
    }

    #[test]
    fn only_recognised_languages_are_coloured() {
        use std::path::Path;
        for code in [
            "a.rs", "a.py", "a.c", "a.h", "a.cpp", "a.js", "a.ts", "a.go", "a.java", "a.toml",
            "a.sh", "a.yaml", "a.html", "a.css", "a.json", "a.sql", "a.lua", "a.rb",
        ] {
            assert!(
                highlights_code(Path::new(code)),
                "{code} should be coloured"
            );
        }
        // Prose, data and the unknown must be left alone. The tokenizer has one
        // keyword list covering every language, so running it over a text file
        // lights up ordinary English words like `is`, `not` and `use`.
        for plain in [
            "a.txt",
            "a.md",
            "a.log",
            "a.csv",
            "a.rst",
            "a.rs.bak",
            "noextension",
            "a.unknownext",
            "Cargo.lock",
        ] {
            assert!(
                !highlights_code(Path::new(plain)),
                "{plain} is not code and must not be coloured"
            );
        }
    }
}
