//! Colouring one line.
//!
//! The editor only ever asks about the lines inside the viewport, about forty
//! of them, so this is a plain left-to-right scan of a single `&str`: no
//! lookahead, no second pass, no allocation beyond the runs themselves. Speed
//! is real but the budget is tiny, so clarity wins wherever the two disagree.
//!
//! The invariant that outranks every colouring rule: **the runs tile the line
//! exactly.** The caller places each run after the last one, in order, so a
//! dropped or doubled character shows up as visibly scrambled text, not as a
//! slightly wrong colour. Every path below therefore ends a run at a character
//! boundary and hands back a slice of the original line, never a rebuilt one,
//! and never an empty slice.
//!
//! Everything is driven from one byte cursor. `i` is always on a character
//! boundary: ASCII bytes are one byte each and UTF-8 continuation bytes are
//! never mistaken for the start of a token, so the cursor only ever lands
//! somewhere a `line[i..]` slice is legal.

use std::collections::HashSet;
use std::sync::OnceLock;

/// What a run of characters in a line is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    /// Ordinary code.
    Plain,
    /// A line comment, from the comment marker to end of line.
    Comment,
    /// A quoted string, quotes included.
    Str,
    /// A numeric literal.
    Number,
    /// A reserved word.
    Keyword,
}

/// Reserved words across the languages this editor claims to highlight.
///
/// The union of the words of every language the file picker will hand to the
/// editor, which is deliberately a union rather than a per-language table: a
/// keyword list is a few hundred bytes of `&'static str` and the highlighter
/// would rather colour `match` in a Python file than argue about grammars.
/// A word being reserved somewhere does not make it reserved here — it only
/// makes it a different colour.
///
/// Words are listed the way their language spells them, which is lowercase for
/// nearly all of them even when the language shouts (SQL, and C macros). The
/// uppercase spelling of every word is folded in when the lookup set is built,
/// so the table stays readable and both cases still colour.
// The table is skipped by rustfmt on purpose: one word per line would be six
// hundred lines of nothing to read, and the grouping by language is the part
// that makes the list checkable by eye.
#[rustfmt::skip]
pub const KEYWORDS: &[&str] = &[
    // Rust.
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "union", "use", "where", "while",
    // Python.
    "and", "assert", "as", "class", "def", "del", "elif", "except", "False", "finally", "from",
    "global", "import", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "True",
    "try", "with", "yield",
    // Lua.
    "do", "elseif", "end", "for", "function", "goto", "if", "in", "local", "nil", "or", "repeat",
    "then", "until", "while",
    // SQL.
    "all", "alter", "and", "as", "avg", "between", "by", "case", "column", "commit", "constraint",
    "count", "create", "cross", "default", "delete", "desc", "distinct", "drop", "else", "end",
    "exists", "foreign", "from", "full", "group", "having", "in", "index", "inner", "insert",
    "into", "is", "join", "key", "left", "like", "limit", "max", "min", "natural", "not", "null",
    "offset", "on", "or", "order", "outer", "primary", "references", "returning", "right", "rollback",
    "select", "set", "sum", "table", "then", "transaction", "union", "unique", "update", "using",
    "values", "view", "when", "where", "with", "begin",
    // POSIX shell.
    "case", "do", "done", "elif", "else", "esac", "fi", "for", "if", "in", "then", "until",
    "while", "function", "break", "continue",
    // C, C++.
    "alignas", "alignof", "auto", "bool", "break", "case", "catch", "char", "class", "concept",
    "const", "consteval", "constinit", "constexpr", "continue", "decltype", "default", "delete",
    "do", "double", "else", "enum", "explicit", "export", "extern", "false", "final", "float",
    "for", "friend", "goto", "if", "inline", "int", "long", "mutable", "namespace", "new", "noexcept",
    "nullptr", "operator", "override", "private", "protected", "public", "register", "reinterpret_cast",
    "requires", "return", "short", "signed", "sizeof", "static", "static_assert", "struct", "switch",
    "template", "this", "throw", "true", "try", "typedef", "typeid", "typename", "union",
    "unsigned", "using", "virtual", "void", "volatile", "while",
    // C#.
    "abstract", "as", "async", "await", "base", "checked", "delegate", "dynamic", "event",
    "explicit", "file", "fixed", "foreach", "get", "implicit", "init", "internal", "is", "lock",
    "nameof", "namespace", "null", "out", "params", "partial", "record", "required", "scoped",
    "set", "sizeof", "stackalloc", "string", "typeof", "unchecked", "value", "var", "when",
    "where", "yield",
    // JavaScript and TypeScript.
    "any", "as", "asserts", "async", "await", "boolean", "debugger", "declare", "delete", "export",
    "extends", "from", "function", "implements", "import", "infer", "instanceof", "interface",
    "keyof", "never", "number", "object", "package", "satisfies", "string", "symbol", "typeof",
    "undefined", "unknown", "var", "void", "yield",
    // Go, including the predeclared names most editors colour like keywords.
    "any", "append", "bool", "byte", "cap", "chan", "close", "complex64", "complex128", "copy",
    "defer", "delete", "error", "fallthrough", "float32", "float64", "func", "go", "goto", "int",
    "int8", "int16", "int32", "int64", "len", "make", "map", "min", "max", "panic", "print",
    "println", "range", "recover", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64",
    "uintptr",
    // Java, Kotlin, Scala.
    "abstract", "annotation", "catch", "companion", "constructor", "data", "extends", "final",
    "finally", "implicit", "implements", "import", "init", "infix", "inline", "instanceof",
    "interface", "internal", "lateinit", "native", "object", "open", "out", "package", "permits",
    "private", "protected", "public", "sealed", "static", "strictfp", "super", "synchronized",
    "this", "throw", "throws", "trait", "transient", "try", "typealias", "val", "var", "vararg",
    "void", "yield", "given", "opaque",
    // Ruby, PHP.
    "alias", "and", "begin", "class", "clone", "def", "default", "defined",
    "do", "echo", "elsif", "empty", "end", "enddeclare", "endfor", "endforeach", "endif",
    "endswitch", "endwhile", "enum", "fn", "foreach", "global", "goto", "include", "include_once",
    "insteadof", "isset", "list", "match", "mod", "module", "new", "next", "nil", "or", "print",
    "readonly", "redo", "require", "require_once", "rescue",
    "retry", "self", "static", "throw", "trait", "undef", "unless", "until", "use", "var", "while",
    "xor", "yield", "int", "float", "bool", "iterable", "mixed", "never", "null",
    // TOML, INI, YAML.
    "none", "no", "off", "on", "yes", "null", "true", "false",
    // CSS.
    "charset", "font-face", "import", "initial", "inherit", "keyframes", "media", "not", "only",
    "revert", "root", "supports", "unset",
];

/// The lookup set behind [`KEYWORDS`], built once.
///
/// A linear scan of the list per word would be a hundred comparisons on every
/// identifier of every visible line, every frame, and nothing else in this
/// file is allowed to be that slow. The set is built the first time a word
/// needs it and is immutable afterwards, so a frame is a relaxed atomic load
/// and a hash.
static KEYWORD_SET: OnceLock<HashSet<String>> = OnceLock::new();

/// Whether `word` is a reserved word.
///
/// Two spellings per word, not a case fold: the word as listed and the word
/// shouted. That is what SQL needs, and `letter` still fails the lookup
/// either way. A word in no other case, `Select` for one, is not a keyword,
/// because a per-language table is exactly what this deliberately is not.
fn is_keyword(word: &str) -> bool {
    KEYWORD_SET
        .get_or_init(|| {
            let mut set = HashSet::with_capacity(KEYWORDS.len() * 2);
            for &word in KEYWORDS {
                set.insert(word.to_owned());
                let shouted = word.to_uppercase();
                if shouted != word {
                    set.insert(shouted);
                }
            }
            set
        })
        .contains(word)
}

/// The run being collected, as a byte range into the line.
///
/// A range with `start == end` means no run is in progress, which is how the
/// scanner says "nothing collected yet" without an `Option` to unwrap at every
/// step. `Open::idle` is the only way to make one.
struct Open {
    start: usize,
    end: usize,
    token: Token,
    /// Set for an identifier, which is always its own run: `x` in `x;` stays
    /// `Plain` on its own even though `;` is `Plain` too, so the edge of a
    /// word is where the caller expects it and a keyword is never smeared into
    /// its surroundings. Every other class is mergeable.
    word: bool,
}

impl Open {
    /// No run in progress.
    fn idle() -> Open {
        Open {
            start: 0,
            end: 0,
            token: Token::Plain,
            word: false,
        }
    }

    /// Whether a run is really in progress rather than idle.
    fn live(&self) -> bool {
        self.start < self.end
    }
}

/// Moves the collected run into `runs` and clears the slot.
///
/// An idle or empty range is dropped rather than pushed: a zero-width run in
/// the middle of a line is a glyph with no width at a made-up position, and the
/// caller has no way to want one. This is also the only place that can decide a
/// run is real, so "never emit an empty `String`" is checked once.
fn push(runs: &mut Vec<(String, Token)>, open: &mut Open, line: &str) {
    if open.live() {
        runs.push((line[open.start..open.end].to_owned(), open.token));
    }
    *open = Open::idle();
}

/// Adds one run of `token` over `line[start..end]`, merging it into the run
/// before it when the two are the same class and neither is an identifier.
///
/// Merging is what turns ` = ` into one run instead of three, and a Str
/// immediately followed by another Str is one colour either way.
fn emit(
    runs: &mut Vec<(String, Token)>,
    open: &mut Open,
    line: &str,
    start: usize,
    end: usize,
    token: Token,
    word: bool,
) {
    if end <= start {
        return;
    }
    if !word && !open.word && open.token == token && open.live() {
        open.end = end;
        return;
    }
    push(runs, open, line);
    *open = Open {
        start,
        end,
        token,
        word,
    };
}

/// Length in bytes of the character at `at`, which must be a boundary.
fn char_len(line: &str, at: usize) -> usize {
    if line.as_bytes()[at] < 0x80 {
        // ASCII is one byte by definition, and it is nearly all of the text.
        1
    } else {
        // `at` is a boundary, so there is a whole character here to measure.
        // The fallback is unreachable and exists so that nothing here can
        // panic; every caller is inside the line.
        line[at..].chars().next().map_or(1, char::len_utf8)
    }
}

/// Scans a quoted string from `from`, just past its opening quote, and says where
/// it ended and whether it was closed.
///
/// An unterminated string is the normal state of a line while somebody is halfway
/// through typing one, so what is returned is the end of the line and `false`, and
/// the caller colours the rest of the line as the string and keeps the text: the
/// alternative, giving up and colouring the tail as code, flickers colours under
/// the caret, and looping until a close quote is found would never terminate.
fn scan_quoted(line: &str, from: usize, quote: u8) -> (usize, bool) {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            // A backslash means the next character is literal, so step over
            // that character whole. Stepping one byte would split a
            // multi-byte character in half; not stepping at all is the bug
            // this branch exists to prevent, because `\"` must not end the
            // string.
            i += 1;
            if i < bytes.len() {
                i += char_len(line, i);
            }
        } else if b == quote {
            return (i + 1, true);
        } else {
            i += char_len(line, i);
        }
    }
    (bytes.len(), false)
}

/// Scans a triple-quoted string from `from`, just past its opening three quotes.
fn scan_triple(line: &str, from: usize, quote: u8) -> (usize, bool) {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            if i < bytes.len() {
                i += char_len(line, i);
            }
        } else if b == quote && bytes.get(i + 1) == Some(&quote) && bytes.get(i + 2) == Some(&quote)
        {
            return (i + 3, true);
        } else {
            i += char_len(line, i);
        }
    }
    (bytes.len(), false)
}

/// End of the numeric literal that starts at `start`.
///
/// Deliberately wider than the digits it starts with, because a number is not
/// a number to a highlighter, it is a run of characters only a real lexer
/// could take apart, and the run has to be unbroken or the colour flickers
/// mid-literal. Letters, `_` and `.` are eaten freely, which is what makes
/// `0x1F`, `123_456` and `1.5` single runs with no prefix table. A `+` or `-`
/// is eaten only directly behind an `e`/`E`, because that is the one place
/// where a sign cannot be a subtraction: `1e-9` is one number and `1-9` is
/// two.
fn number_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
            i += 1;
        } else if (b == b'+' || b == b'-') && i > start && matches!(bytes[i - 1], b'e' | b'E') {
            // `i > start` cannot fail here: a sign is only reached once
            // something else of the literal has been eaten, and the first
            // character is a digit. It is there so the index below is never
            // computed at `start`.
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// The longest line, in bytes, that is coloured. Past this a line is drawn plain.
pub const MAX_HIGHLIGHT_LINE: usize = 20_000;

/// What a line inherits from the lines above it.
///
/// A block comment or a triple-quoted string opens on one line and closes on a
/// later one, so colouring a line needs to know whether the text before it left
/// one open. That is all the lexer carries from line to line, which is why the
/// editor can keep one of these per line start and colour any line by looking up
/// its state and scanning only that line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    /// Ordinary code.
    #[default]
    Normal,
    /// Inside a block comment.
    Block,
    /// Inside a triple-quoted string, closed by three of this quote.
    Triple(u8),
    /// Inside a template literal, which is a backtick string that may span lines.
    Template,
}

/// What a language can carry across a line break.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lang {
    /// The line-comment marker, or empty for none.
    pub line: &'static str,
    /// The block-comment delimiters, if the language has them.
    pub block: Option<(&'static str, &'static str)>,
    /// Whether three quotes open a string that may span lines.
    pub triple: bool,
    /// Whether a backtick string may span lines.
    pub template: bool,
}

impl Lang {
    /// A language with only a line comment: nothing it does crosses a line break.
    pub const fn line_only(line: &'static str) -> Lang {
        Lang {
            line,
            block: None,
            triple: false,
            template: false,
        }
    }
}

/// Splits one line into coloured runs that concatenate back to exactly `line`.
///
/// `comment` is the line-comment marker for the language, e.g. `"//"`, `"#"` or
/// `"--"`. It is matched whole, so `"//"` does not fire on a lone `/`. A
/// single-character marker is accepted, because `#` is the comment token for
/// more languages than any other; an empty one means the language has no
/// line comments and nothing is ever a comment.
///
/// Never fails and never panics, whatever the input.
#[cfg_attr(not(test), allow(dead_code))]
pub fn tokenize(line: &str, comment: &str) -> Vec<(String, Token)> {
    let mut runs: Vec<(String, Token)> = Vec::new();
    let mut open = Open::idle();
    scan(
        line,
        comment,
        &Lang::line_only(""),
        State::Normal,
        &mut |a, b, token, word| emit(&mut runs, &mut open, line, a, b, token, word),
    );
    push(&mut runs, &mut open, line);
    runs
}

/// [`tokenize`] for a language with constructs that cross lines: the runs of one
/// line, given the state it starts in, and the state the next line starts in.
pub fn tokenize_with(line: &str, lang: &Lang, state: State) -> (Vec<(String, Token)>, State) {
    // A line this long is drawn plain. Colouring it means scanning every character
    // and building a run for each token every frame, which for a minified file is
    // a million characters and a quarter of a second, to colour text nobody reads
    // one token at a time. The state carries through unchanged.
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
    let out = scan(line, lang.line, lang, state, &mut |a, b, token, word| {
        emit(&mut runs, &mut open, line, a, b, token, word)
    });
    push(&mut runs, &mut open, line);
    (runs, out)
}

/// The state the next line starts in, without building any runs.
///
/// The same scan as [`tokenize_with`] with the colouring thrown away, so the two
/// can never disagree about where a comment ends.
pub fn advance(line: &str, lang: &Lang, state: State) -> State {
    if line.len() > MAX_HIGHLIGHT_LINE {
        return state;
    }
    scan(line, lang.line, lang, state, &mut |_, _, _, _| {})
}

/// The one scan behind all three. Reports each run as `(start, end, token,
/// is_word)` to `sink`, left to right, and returns the state the line leaves
/// behind.
fn scan(
    line: &str,
    marker: &str,
    lang: &Lang,
    state: State,
    sink: &mut dyn FnMut(usize, usize, Token, bool),
) -> State {
    let bytes = line.as_bytes();
    let mut i = 0usize;

    // A line that starts inside something first has to find where that ends.
    match state {
        State::Normal => {}
        State::Block => {
            let close = lang.block.map_or("", |b| b.1);
            match line.find(close).filter(|_| !close.is_empty()) {
                Some(k) => {
                    i = k + close.len();
                    sink(0, i, Token::Comment, false);
                }
                None => {
                    sink(0, bytes.len(), Token::Comment, false);
                    return State::Block;
                }
            }
        }
        State::Triple(q) => {
            let (end, closed) = scan_triple(line, 0, q);
            sink(0, end, Token::Str, false);
            if !closed {
                return State::Triple(q);
            }
            i = end;
        }
        State::Template => {
            let (end, closed) = scan_quoted(line, 0, b'`');
            sink(0, end, Token::Str, false);
            if !closed {
                return State::Template;
            }
            i = end;
        }
    }

    while i < bytes.len() {
        // 1. The comment marker ends the line.
        if !marker.is_empty() && line[i..].starts_with(marker) {
            sink(i, bytes.len(), Token::Comment, false);
            return State::Normal;
        }

        // 2. A block comment opens, and runs to its close or to the end of the
        //    line — in which case the next line starts inside it.
        if let Some((open, close)) = lang.block
            && line[i..].starts_with(open)
        {
            let from = i + open.len();
            match line[from..].find(close) {
                Some(k) => {
                    let end = from + k + close.len();
                    sink(i, end, Token::Comment, false);
                    i = end;
                    continue;
                }
                None => {
                    sink(i, bytes.len(), Token::Comment, false);
                    return State::Block;
                }
            }
        }

        let b = bytes[i];

        // 3. Three quotes open a string that may span lines, in the languages that
        //    have one. Checked before a lone quote is looked at, which is only a
        //    string here when it is one of the three.
        if lang.triple
            && (b == b'"' || b == b'\'')
            && bytes.get(i + 1) == Some(&b)
            && bytes.get(i + 2) == Some(&b)
        {
            let (end, closed) = scan_triple(line, i + 3, b);
            sink(i, end, Token::Str, false);
            if !closed {
                return State::Triple(b);
            }
            i = end;
            continue;
        }

        // 4. A quote opens a string. The marker never gets a look inside one:
        //    the scan runs left to right, so a string that started earlier has
        //    already been consumed whole by the time the cursor could reach a
        //    marker inside it.
        //
        //    A single quote is deliberately not one of them. In the languages
        //    where `'` does open a string, it is rare, and the cost of treating
        //    it as one is high and very visible: a Rust lifetime, a shell
        //    parameter, and above all an apostrophe in ordinary English all run
        //    to the end of the line as an unterminated string. Losing a rarely
        //    used colour is a fair trade for not colouring half of every
        //    comment and every doc line.
        if b == b'"' || b == b'`' {
            let (end, closed) = scan_quoted(line, i + 1, b);
            sink(i, end, Token::Str, false);
            // A backtick string carries on to the next line only where the
            // language lets it; everywhere else an open one ends with its line.
            if !closed && b == b'`' && lang.template {
                return State::Template;
            }
            i = end;
            continue;
        }

        // 5. A digit opens a number.
        if b.is_ascii_digit() {
            let end = number_end(bytes, i);
            sink(i, end, Token::Number, false);
            i = end;
            continue;
        }

        // 6. A letter or `_` opens a word, which is a run of word characters.
        //    The digit case is above, so a digit inside a word belongs to the
        //    word and is not a second run.
        if b == b'_' || b.is_ascii_alphabetic() {
            let mut end = i + 1;
            while end < bytes.len() && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric()) {
                end += 1;
            }
            let word = &line[i..end];
            let token = if is_keyword(word) {
                Token::Keyword
            } else {
                Token::Plain
            };
            sink(i, end, token, true);
            i = end;
            continue;
        }

        // 7. Anything else is one character of code. Consecutive ones merge,
        //    which is what stops ` = ` and the spaces in `let x` from being
        //    three runs each.
        let end = i + char_len(line, i);
        sink(i, end, Token::Plain, false);
        i = end;
    }
    State::Normal
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runs laid end to end, which is what the caller does to draw them.
    fn joined(runs: &[(String, Token)]) -> String {
        let mut text = String::new();
        for (run, _) in runs {
            text.push_str(run);
        }
        text
    }

    /// The run classes with repeats dropped. Two adjacent `Plain` runs are
    /// just plain code, so this is how the shape of a line reads.
    fn collapsed(runs: &[(String, Token)]) -> Vec<Token> {
        let mut out: Vec<Token> = Vec::new();
        for (_, token) in runs {
            if out.last() != Some(token) {
                out.push(*token);
            }
        }
        out
    }

    /// The texts of the runs, for talking about a line without its classes.
    fn texts(runs: &[(String, Token)]) -> Vec<&str> {
        runs.iter().map(|(run, _)| run.as_str()).collect()
    }

    /// Tokenizes and asserts the property that outranks the colours: the runs
    /// are the line, back to back, with nothing empty in between.
    fn check(line: &str, comment: &str) -> Vec<(String, Token)> {
        let runs = tokenize(line, comment);
        assert_eq!(joined(&runs), line, "runs do not reproduce {line:?}");
        for (run, _) in &runs {
            assert!(!run.is_empty(), "empty run in {runs:?}");
        }
        assert_eq!(
            runs.is_empty(),
            line.is_empty(),
            "an empty line has no runs and a non-empty line has at least one"
        );
        runs
    }

    #[test]
    fn empty_line_has_no_runs() {
        assert!(check("", "//").is_empty());
        assert!(check("", "").is_empty());
        assert!(check("", "#").is_empty());
    }

    #[test]
    fn spaces_only_is_one_plain_run() {
        let runs = check("   ", "//");
        assert_eq!(texts(&runs), vec!["   "]);
        assert_eq!(runs[0].1, Token::Plain);
    }

    #[test]
    fn a_let_binding_reads_as_keyword_plain_number_plain() {
        let runs = check("let x = 1;", "//");
        assert_eq!(
            collapsed(&runs),
            vec![Token::Keyword, Token::Plain, Token::Number, Token::Plain]
        );
        // `x` is a word, and a word is its own run even though it is the same
        // colour as the ` = ` around it, so the line is six runs and not four.
        assert_eq!(
            texts(&runs),
            vec!["let", " ", "x", " = ", "1", ";"],
            "unexpected run split"
        );
        assert_eq!(
            runs.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            vec![
                Token::Keyword,
                Token::Plain,
                Token::Plain,
                Token::Plain,
                Token::Number,
                Token::Plain
            ]
        );
    }

    #[test]
    fn a_slash_comment_takes_the_rest_of_the_line() {
        let runs = check("// hi", "//");
        assert_eq!(texts(&runs), vec!["// hi"]);
        assert_eq!(runs[0].1, Token::Comment);
    }

    #[test]
    fn a_hash_comment_takes_the_rest_of_the_line() {
        // `#` is a single character and is accepted, because it is the comment
        // token for more languages than any other marker in this editor.
        let runs = check("# hi", "#");
        assert_eq!(texts(&runs), vec!["# hi"]);
        assert_eq!(runs[0].1, Token::Comment);

        // A shebang, the two-character hash marker case.
        let runs = check("#!/bin/sh", "#!");
        assert_eq!(texts(&runs), vec!["#!/bin/sh"]);
        assert_eq!(runs[0].1, Token::Comment);
    }

    #[test]
    fn a_comment_marker_may_be_empty_at_the_end_of_a_line() {
        // The marker with nothing after it still has to be one comment run.
        let runs = check("//", "//");
        assert_eq!(texts(&runs), vec!["//"]);
        assert_eq!(runs[0].1, Token::Comment);
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        // `# ` is a usable marker and it really does occur inside the string,
        // at its first character, so the cursor has to step over the whole
        // string for this to pass. A double quote opens a string; a single one
        // deliberately does not, which is what keeps an apostrophe in prose
        // from colouring the rest of the line.
        let runs = check("puts \"# not a comment\"", "# ");
        assert_eq!(
            collapsed(&runs),
            vec![Token::Plain, Token::Str],
            "the hash inside the string was treated as a comment"
        );
        assert_eq!(texts(&runs), vec!["puts", " ", "\"# not a comment\""]);
    }

    #[test]
    fn an_apostrophe_does_not_open_a_string() {
        // The single most damaging thing a naive tokenizer does to a text file:
        // `don't` opens an unterminated string and colours the rest of the line.
        let runs = check("don't stop", "#");
        assert_eq!(
            collapsed(&runs),
            vec![Token::Plain],
            "an apostrophe in ordinary text must not start a string"
        );
        assert_eq!(joined(&runs), "don't stop");

        // And a Rust lifetime, which is the same character for a different reason.
        let runs = check("fn f<'a>(x: &'a str) {}", "//");
        assert!(
            !runs.iter().any(|(_, t)| *t == Token::Str),
            "a lifetime was taken for a string"
        );
        assert_eq!(joined(&runs), "fn f<'a>(x: &'a str) {}");
    }

    #[test]
    fn a_comment_marker_inside_a_string_is_not_a_comment() {
        // The same claim, with a marker that really is in the string, so the
        // test cannot pass by accident. The string has to be consumed whole
        // before the cursor ever reaches the marker inside it.
        let runs = check("puts \"a // b\";", "//");
        assert_eq!(
            collapsed(&runs),
            vec![Token::Plain, Token::Str, Token::Plain]
        );
        assert_eq!(texts(&runs), vec!["puts", " ", "\"a // b\"", ";"]);
    }

    #[test]
    fn an_unterminated_string_keeps_its_text() {
        let line = "let s = \"abc";
        let runs = check(line, "//");
        assert_eq!(runs.last().map(|(_, t)| *t), Some(Token::Str));
        assert_eq!(texts(&runs).last().copied(), Some("\"abc"));
    }

    #[test]
    fn a_quote_alone_is_a_string_and_not_a_hang() {
        let runs = check("\"", "//");
        assert_eq!(runs, vec![("\"".to_owned(), Token::Str)]);
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        let runs = check("\"a\\\"b\"", "//");
        assert_eq!(runs, vec![("\"a\\\"b\"".to_owned(), Token::Str)]);
    }

    #[test]
    fn a_backslash_at_the_end_of_a_line_keeps_its_text() {
        // The escape has nothing to escape, so the string is unterminated, but
        // the backslash is the caller's character and must survive.
        let line = "\"a\\";
        let runs = check(line, "//");
        assert_eq!(runs, vec![(line.to_owned(), Token::Str)]);
    }

    #[test]
    fn an_escape_may_hide_a_multibyte_character() {
        // The escaped character is stepped over whole, so the é cannot end up
        // half inside and half outside the string run.
        let line = "\"\\é\"";
        let runs = check(line, "//");
        assert_eq!(runs, vec![(line.to_owned(), Token::Str)]);
    }

    #[test]
    fn numbers_stay_one_run() {
        for n in [
            "1",
            "0",
            "0x1F",
            "0b1010",
            "123_456",
            "1.5",
            "1e-9",
            "1.5e+3",
            "1E9",
            "3.14e-2",
            "0xDEAD_BEEF",
        ] {
            let runs = check(n, "//");
            assert_eq!(texts(&runs), vec![n], "{n} was split");
            assert_eq!(runs[0].1, Token::Number, "{n} is not a number run");
        }
    }

    #[test]
    fn a_sign_that_is_not_an_exponent_belongs_to_the_operator() {
        let runs = check("1-9", "//");
        assert_eq!(texts(&runs), vec!["1", "-", "9"]);
        assert_eq!(runs[0].1, Token::Number);
        assert_eq!(runs[1].1, Token::Plain);
        assert_eq!(runs[2].1, Token::Number);
    }

    #[test]
    fn an_identifier_with_digits_is_not_split() {
        let runs = check("handler_42", "//");
        assert_eq!(runs, vec![("handler_42".to_owned(), Token::Plain)]);
    }

    #[test]
    fn a_keyword_prefix_is_not_a_keyword() {
        let runs = check("let letter", "//");
        assert_eq!(texts(&runs), vec!["let", " ", "letter"]);
        assert_eq!(runs[0].1, Token::Keyword);
        assert_eq!(runs[2].1, Token::Plain);
    }

    #[test]
    fn a_marker_must_match_whole_not_by_its_first_character() {
        // `//` does not match a lone `/`, so the division stays part of the
        // code and only the `//` starts a comment. There is no digit here, so
        // the whole prefix collapses to one plain run.
        let runs = check("a / b; // hi", "//");
        assert_eq!(
            collapsed(&runs),
            vec![Token::Plain, Token::Comment],
            "a single slash is an operator, not half a comment marker"
        );
        assert_eq!(joined(&runs), "a / b; // hi");

        // The mirror image: a one-character marker matches on its own, which is
        // why `#include` reads as a comment in a C file. That is the accepted
        // cost of colouring `#` comments in the dozen languages that use them.
        let runs = check("#include <stdio.h>", "#");
        assert_eq!(collapsed(&runs), vec![Token::Comment]);
    }

    #[test]
    fn an_empty_marker_is_ignored() {
        let line = "// hi";
        let runs = check(line, "");
        assert_eq!(collapsed(&runs), vec![Token::Plain]);
        assert_eq!(joined(&runs), line);
    }

    #[test]
    fn trailing_whitespace_survives() {
        let line = "let x = 1;   ";
        let runs = check(line, "//");
        assert!(line.ends_with(' '));
        assert!(
            runs.last().is_some_and(|(run, _)| run.ends_with(";   ")),
            "trailing spaces were dropped: {runs:?}"
        );
    }

    #[test]
    fn unicode_round_trips() {
        let line = "let s = \"héllo wörld\";";
        let runs = check(line, "//");
        assert_eq!(runs[0], ("let".to_owned(), Token::Keyword));
        assert_eq!(runs[4], ("\"héllo wörld\"".to_owned(), Token::Str));
        assert_eq!(runs[5], (";".to_owned(), Token::Plain));
    }

    #[test]
    fn multibyte_characters_outside_a_string_are_code() {
        // A character of several bytes must be stepped over whole, or the
        // slice after it would not be on a character boundary.
        let runs = check("π = 3; // π", "//");
        assert_eq!(runs[0], ("π = ".to_owned(), Token::Plain));
        assert_eq!(runs[1], ("3".to_owned(), Token::Number));
        assert_eq!(runs.last(), Some(&("// π".to_owned(), Token::Comment)));
        let runs = check("🙂🙂", "//");
        assert_eq!(runs, vec![("🙂🙂".to_owned(), Token::Plain)]);
    }

    #[test]
    fn a_long_line_does_not_overflow_the_stack() {
        // The scanner is a loop, so the only way to fail this is a recursive
        // descent that recurses once per character.
        let mut line = String::new();
        for i in 0..50_000 {
            line.push_str("let s = \"a\\\"b\"; x = ");
            line.push_str(&i.to_string());
            line.push(';');
        }
        let runs = check(&line, "//");
        assert!(runs.len() > 300_000, "only {} runs", runs.len());
    }

    #[test]
    fn a_line_of_backslashes_does_not_overflow_the_stack() {
        // Every other character opens an escape, so a scanner that recursed per
        // escape would run out of stack on this line. The parity decides
        // whether the closing quote closes anything.
        let even = format!("\"{}\"", "\\".repeat(100_000));
        assert_eq!(check(&even, "//").len(), 1, "the quote closes the string");
        let odd = format!("\"{}\"", "\\".repeat(99_999));
        assert_eq!(
            check(&odd, "//").len(),
            1,
            "the quote is escaped, so the string runs to the end of the line"
        );
    }

    #[test]
    fn keyword_lookup_ignores_case() {
        // SQL is written in capitals, and a highlighter that misses it looks
        // broken, so the shouted spelling of a keyword is the same word. Only
        // the two cases are folded in, so a PascalCase name is not one.
        for word in ["let", "LET", "select", "SELECT"] {
            let line = format!("{word} 1");
            let runs = check(&line, "//");
            assert_eq!(runs[0].1, Token::Keyword, "{word} is not a keyword");
        }
        for word in ["letter", "LETTER", "lettuce", "Select", "unselected"] {
            let line = format!("{word} 1");
            let runs = check(&line, "//");
            assert_eq!(runs[0].1, Token::Plain, "{word} is a keyword");
        }
    }

    #[test]
    fn the_keyword_list_is_usable_as_a_table() {
        assert!(!KEYWORDS.is_empty());
        for &word in KEYWORDS {
            assert!(!word.is_empty(), "an empty keyword matches everything");
            assert!(word.is_ascii(), "{word} needs no case folding");
        }
    }

    #[test]
    fn adjacent_strings_are_one_colour() {
        let runs = check("\"a\"\"b\"", "//");
        assert_eq!(collapsed(&runs), vec![Token::Str]);
        assert_eq!(joined(&runs), "\"a\"\"b\"");
    }

    #[test]
    fn awkward_lines_round_trip_under_every_marker() {
        // The property test: whatever the line and whatever the marker, the
        // runs have to be the line.
        const LINES: &[&str] = &[
            "",
            " ",
            "\t\t",
            "\r",
            "let x = 1;\r",
            "a",
            "_",
            "__x1__",
            "0",
            "...",
            "a...b",
            "\"\"",
            "''",
            "``",
            "\"\"\"",
            "'''",
            "\"\\",
            "\\",
            "\\\\",
            "e",
            "1e",
            "1e+",
            "1e-",
            "1.",
            "1..2",
            "0x",
            "1_2_3",
            "#!/bin/sh",
            "a#b",
            "#a#b#",
            "//",
            "///",
            "/*",
            "/*/",
            "*/",
            "--",
            "----",
            "a--b",
            "let s = \"héllo wörld\";",
            "π = 3;",
            "🙂=1;",
            "printf('%s', x);",
            "SELECT * FROM t WHERE a = 1;",
            "def f(x): return x # go",
            "end",
            "let s = \"emoji 🙂 in a string\";",
        ];
        for &line in LINES {
            for marker in ["", "#", "//", "--", "/*", "##", "'", "///", "\"", "🙂"] {
                check(line, marker);
            }
        }
    }

    #[test]
    fn a_comment_marker_longer_than_the_line_is_simply_never_seen() {
        let line = "// x";
        let runs = check(line, "////");
        assert_eq!(
            collapsed(&runs),
            vec![Token::Plain],
            "a marker that does not fit must not match"
        );
        assert_eq!(joined(&runs), line);
    }
}
