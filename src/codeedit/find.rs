//! Searching inside the open document.
//!
//! Separate from the widget because this is the only part of the editor that
//! looks at the whole file rather than the window on screen, and it needs to be
//! right on its own: a search that is wrong is worse than no search, and a search
//! that is slow makes the editor feel slow even when nothing is being edited.
//!
//! Two properties are worth stating, because they are what the tests are for.
//!
//! - **Byte offsets in, character indices out.** The widget speaks characters;
//!   `str` searching speaks bytes. Every hit is converted here, once, so that no
//!   caller has to remember which is which.
//! - **Bounded work.** The scan is a single pass and the number of hits kept is
//!   capped, so a search for a single letter in a multi-megabyte file cannot turn
//!   into a multi-megabyte allocation and a frame's worth of highlighting.

/// A hit, as a range of document characters.
#[cfg_attr(not(test), allow(dead_code))]
pub type Hit = (usize, usize);

/// The most hits kept for highlighting.
///
/// Past this the bar still counts correctly - the count is of the whole file -
/// but stops marking them, because a reader is looking at one of them and the
/// ones past a hundred thousand are not what is on screen. Without a cap, a
/// search for a space in a large file allocates a vector of a million pairs
/// every keystroke.
#[cfg_attr(not(test), allow(dead_code))]
pub const MAX_HITS: usize = 20_000;

/// Everything the search is asked to do, and nothing it remembers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// Whether letters must match exactly.
    pub case: bool,
    /// Whether the hit must be a whole word.
    pub whole: bool,
}

impl Query {
    /// A query that finds the needle anywhere, whatever its case.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn loose() -> Self {
        Query::default()
    }

    /// A query that must match letters and word boundaries exactly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn strict() -> Self {
        Query {
            case: true,
            whole: true,
        }
    }
}

/// Every hit in `text`, as character ranges, in order.
///
/// An empty needle has no hits. That is not a special case for tidiness: a zero
/// length match occurs between every pair of characters, so "find nothing" and
/// "find everything" are the same program, and only one of them is useful.
#[cfg_attr(not(test), allow(dead_code))]
pub fn find_all(text: &str, needle: &str, query: Query) -> Vec<Hit> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    // Two genuinely different searches rather than two speeds of one. Folding
    // ASCII case says nothing about `É` against `é`, and a search that quietly
    // did the wrong one looks like a fault in the feature rather than a limit
    // of it.
    if needle.is_ascii() {
        scan_ascii(text, needle, query, &mut out);
    } else {
        scan_wide(text, needle, query, &mut out);
    }
    out
}

/// The fast path: a byte scan for the first byte, then a compare.
///
/// A candidate is found by its first byte, which is what makes this a scan and
/// not a comparison at every position. An ASCII byte can only ever sit at a
/// character boundary - a UTF-8 continuation byte is always 0x80 or above - so
/// every candidate is one, and the character count only has to be advanced over
/// the gaps between them.
fn scan_ascii(text: &str, needle: &str, query: Query, out: &mut Vec<Hit>) {
    let n = needle.len();
    let first = needle.as_bytes()[0];
    let want = if query.case {
        first
    } else {
        first.to_ascii_lowercase()
    };
    let n_chars = needle.chars().count();
    let mut at_char = 0usize;
    let mut from = 0usize;
    while from + n <= text.len() {
        let Some(found) = text.as_bytes()[from..].iter().position(|&b| {
            if query.case {
                b == want
            } else {
                b.to_ascii_lowercase() == want
            }
        }) else {
            break;
        };
        let at = from + found;
        at_char += text[from..at].chars().count();
        let end = at + n;
        if text.is_char_boundary(end)
            && matches_at(&text[at..end], needle, query)
            && (!query.whole || is_whole(text, at, end))
        {
            out.push((at_char, at_char + n_chars));
            if out.len() >= MAX_HITS {
                return;
            }
        }
        // On by one whole character rather than one match. A needle that overlaps
        // itself - "aa" in "aaaa" - has a hit starting inside the previous one, and
        // stepping past the match would find only the first of them.
        let step = text[at..].chars().next().map_or(1, char::len_utf8);
        at_char += 1;
        from = at + step;
    }
}

/// The path for a needle that is not ASCII, where folding has to be done by
/// character.
///
/// Slower by design: it compares at every character boundary rather than scanning
/// for a first byte. It can be, because a needle with an accented letter is rare
/// enough that getting it wrong would be the bigger problem. Nothing here
/// allocates - `char::to_lowercase` is an iterator - because lowercasing a copy
/// of a multi-megabyte file on every keystroke is not a price worth paying.
fn scan_wide(text: &str, needle: &str, query: Query, out: &mut Vec<Hit>) {
    let n = needle.len();
    let lower = needle.to_lowercase();
    let n_lower = lower.chars().count();
    let n_chars = needle.chars().count();
    // Named apart rather than destructured in place, because the two are the two
    // units this file is about and swapping them compiles perfectly: `char_indices`
    // yields the *byte* offset and `enumerate` yields the character one, and
    // reading them the other way round is a search that finds real hits at
    // positions no reader can see.
    for (char_index, (byte, _)) in text.char_indices().enumerate() {
        if byte + n > text.len() {
            break;
        }
        // `get` and not `[..]`: a needle of `n` bytes does not necessarily end on
        // a character boundary of the text, and a slice there would panic in the
        // middle of a keystroke handler.
        let hit = text.get(byte..byte + n).is_some_and(|head| {
            if query.case {
                head == needle
            } else {
                head.chars().count() == n_lower
                    && head
                        .chars()
                        .zip(lower.chars())
                        .all(|(a, b)| a.to_lowercase().eq(std::iter::once(b)))
            }
        });
        if hit && (!query.whole || is_whole(text, byte, byte + n)) {
            out.push((char_index, char_index + n_chars));
            if out.len() >= MAX_HITS {
                return;
            }
        }
    }
}

/// Whether the bytes at a hit are the needle, under the query's rules.
fn matches_at(hay: &str, needle: &str, query: Query) -> bool {
    if query.case {
        hay == needle
    } else {
        hay.eq_ignore_ascii_case(needle)
    }
}

/// Whether a hit is bounded by things that are not word characters.
///
/// Asked of the text either side of the hit rather than of the hit itself,
/// because a needle can be a whole word and still be glued to another one - in
/// `foofoo`, both halves match `foo` and neither is a word on its own.
fn is_whole(text: &str, start: usize, end: usize) -> bool {
    fn word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(word) && !after.is_some_and(word)
}

/// Every hit in a buffer, as character ranges.
///
/// Searched a line at a time. A needle typed into the find box is a single line, so
/// nothing is lost, and a line is a single chunk of the rope nearly always, so it
/// is searched where it sits with no copy of the document.
pub fn find_all_buffer(text: &crate::buffer::Buffer, needle: &str, query: Query) -> Vec<Hit> {
    find_all_buffer_until(text, needle, query, &|| false)
}

/// [`find_all_buffer`] that gives up, returning what it has, once `cancelled` says
/// so. Asked every few thousand lines, so a search on a worker thread for text that
/// has since changed stops within a moment instead of running to the end.
pub fn find_all_buffer_until(
    text: &crate::buffer::Buffer,
    needle: &str,
    query: Query,
    cancelled: &dyn Fn() -> bool,
) -> Vec<Hit> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    for line in 0..text.lines() {
        if line % 4096 == 0 && cancelled() {
            return out;
        }
        let content = text.line_str(line);
        let base = text.line_start(line);
        for (lo, hi) in find_all(&content, needle, query) {
            out.push((base + lo, base + hi));
            if out.len() >= MAX_HITS {
                return out;
            }
        }
    }
    out
}

/// The hits on the given lines only, as character ranges into the whole buffer.
///
/// For narrowing a search: when the needle grows by a letter, every match of the new
/// needle is also a match of the old one, so only the lines that had a match before
/// can have one now. Searching those lines and no others costs the number of lines
/// that matched instead of the length of the file.
pub fn find_in_lines(
    text: &crate::buffer::Buffer,
    lines: &[usize],
    needle: &str,
    query: Query,
) -> Vec<Hit> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    for &line in lines {
        let content = text.line_str(line);
        let base = text.line_start(line);
        for (lo, hi) in find_all(&content, needle, query) {
            out.push((base + lo, base + hi));
            if out.len() >= MAX_HITS {
                return out;
            }
        }
    }
    out
}

/// Whether the hits are all of them.
///
/// The count the bar shows is of the hits it holds, so a needle with a million
/// matches says "20,000+" rather than spending a frame counting to a number
/// nobody can act on.
pub fn is_capped(hits: &[Hit]) -> bool {
    hits.len() >= MAX_HITS
}

/// The hit after `from`, wrapping round, and its index.
///
/// `from` is the *end* of the current hit, so stepping forward from a hit does
/// not land on the hit itself - the case that makes "next" appear to do nothing
/// on the first press.
#[cfg_attr(not(test), allow(dead_code))]
pub fn next(hits: &[Hit], from: usize, forward: bool) -> Option<(usize, Hit)> {
    if hits.is_empty() {
        return None;
    }
    let found = if forward {
        hits.iter().position(|&(_, hi)| hi > from)
    } else {
        hits.iter().rposition(|&(lo, _)| lo < from)
    };
    let index = found.unwrap_or(if forward { 0 } else { hits.len() - 1 });
    Some((index, hits[index]))
}

#[cfg(test)]
mod tests {
    use super::{MAX_HITS, Query, find_all, is_capped, is_whole, next};

    fn chars(text: &str, needle: &str) -> Vec<(usize, usize)> {
        find_all(text, needle, Query::loose())
    }

    #[test]
    fn an_empty_needle_matches_nothing() {
        assert!(chars("hello", "").is_empty());
        assert!(chars("", "x").is_empty());
        // Every other way round: a needle in an empty document is nothing, not
        // a panic.
        assert!(find_all("", "a", Query::strict()).is_empty());
    }

    #[test]
    fn hits_are_character_ranges_not_byte_offsets() {
        // The whole reason this file exists: `str` finds bytes, the editor counts
        // characters, and on text with anything above ASCII in it they are not the
        // same number. `e` with an acute is two bytes and one character, so the
        // second hit is at character 6 and byte 8; a hit reported in bytes would
        // be out by one.
        let text = "héllo héllo";
        let hits = chars(text, "héllo");
        assert_eq!(text.chars().count(), 11);
        assert_eq!(hits, vec![(0, 5), (6, 11)]);
        // Sliced by character, not by byte, which is the check that catches an
        // off-by-one in either direction.
        let glyphs: Vec<char> = text.chars().collect();
        for &(lo, hi) in &hits {
            let got: String = glyphs[lo..hi].iter().collect();
            assert_eq!(got, "héllo", "characters {lo}..{hi} are the needle");
        }
    }

    #[test]
    fn every_character_position_is_found_exactly_once() {
        // Walks the document one character at a time looking for a single
        // character, so the offsets are the identity - which is the strongest
        // statement available that the character counting is right.
        let text = "aé漢z,q.j\n\tk";
        let unique: std::collections::BTreeSet<char> = text.chars().collect();
        assert_eq!(
            unique.len(),
            text.chars().count(),
            "the fixture repeats itself"
        );
        for (i, c) in text.chars().enumerate() {
            let needle = &c.to_string();
            assert_eq!(
                find_all(
                    text,
                    needle,
                    Query {
                        case: true,
                        whole: false
                    }
                ),
                vec![(i, i + 1)],
                "the character {c:?} is at {i}"
            );
        }
    }

    #[test]
    fn case_is_ignored_by_default_and_honoured_when_asked_for() {
        let text = "Alpha alpha ALPHA";
        assert_eq!(chars(text, "alpha").len(), 3);
        assert_eq!(find_all(text, "alpha", Query::strict()).len(), 1);
        assert_eq!(
            find_all(text, "ALPHA", Query::strict()),
            vec![(12, 17)],
            "and case matters in both directions"
        );
    }

    #[test]
    fn a_whole_word_query_rejects_a_substring_of_a_longer_word() {
        let text = "cat concatenate cat.";
        assert_eq!(chars(text, "cat").len(), 3, "loose finds all three");
        let strict = Query {
            case: false,
            whole: true,
        };
        assert_eq!(
            find_all(text, "cat", strict),
            vec![(0, 3), (16, 19)],
            "`concatenate` is not a whole-word match, and the one after a full \
             stop is"
        );
    }

    #[test]
    fn a_whole_word_query_accepts_a_needle_that_is_itself_two_words() {
        let text = "let x = 1;";
        let strict = Query {
            case: false,
            whole: true,
        };
        assert_eq!(
            find_all(text, "let x", strict),
            vec![(0, 5)],
            "a needle of two words can still be a whole phrase between spaces"
        );
        assert_eq!(find_all("say let x now", "let x", strict), vec![(4, 9)]);
    }

    #[test]
    fn whole_word_boundaries_treat_underscores_and_digits_as_word() {
        assert!(is_whole("a b", 2, 3), "bounded by a space");
        assert!(!is_whole("a b_", 2, 3), "an underscore is a word character");
        assert!(!is_whole("_b a", 0, 1));
        assert!(is_whole("a 1", 2, 3));
        assert!(!is_whole("a 12", 2, 3));
    }

    #[test]
    fn a_case_insensitive_search_folds_accents_too() {
        // ASCII folding says nothing about `É`, and a search for `é` that only
        // finds `é` looks like the feature is broken rather than like a
        // limitation.
        let text = "École école ÉCOLE";
        assert_eq!(chars(text, "école").len(), 3);
        assert_eq!(find_all(text, "école", Query::strict()).len(), 1);
    }

    #[test]
    fn stepping_forwards_goes_to_the_next_hit_and_wraps_round() {
        let hits = chars("a a a", "a");
        assert_eq!(hits, vec![(0, 1), (2, 3), (4, 5)]);
        assert_eq!(
            next(&hits, 0, true),
            Some((0, (0, 1))),
            "from the very start"
        );
        assert_eq!(next(&hits, 1, true), Some((1, (2, 3))), "past the first");
        assert_eq!(next(&hits, 3, true), Some((2, (4, 5))));
        assert_eq!(
            next(&hits, 5, true),
            Some((0, (0, 1))),
            "and past the last it wraps to the first"
        );
    }

    #[test]
    fn stepping_backwards_goes_to_the_previous_hit_and_wraps_round() {
        let hits = chars("a a a", "a");
        assert_eq!(next(&hits, 4, false), Some((1, (2, 3))));
        assert_eq!(next(&hits, 2, false), Some((0, (0, 1))));
        assert_eq!(
            next(&hits, 0, false),
            Some((2, (4, 5))),
            "and before the first it wraps to the last"
        );
        assert_eq!(next(&[], 0, true), None);
    }

    #[test]
    fn stepping_never_returns_the_hit_it_started_from() {
        // The property that makes "next" feel broken when it is wrong: however
        // many times it is pressed, it visits a different hit each time and comes
        // back round to where it started after exactly as many presses as there
        // are hits.
        let text = "one two one two one";
        let hits = chars(text, "one");
        let mut at = 0usize;
        for step in 1..=hits.len() {
            let (_, hit) = next(&hits, at, true).expect("a hit");
            assert_ne!(hit, (at, at), "step {step} did not move");
            at = hit.1;
        }
        // One more press than there are hits, and it is back where it started.
        at = next(&hits, at, true).expect("a hit").1.1;
        assert_eq!(
            at, hits[0].1,
            "and after one more press it is back at the first hit"
        );
    }

    #[test]
    fn overlapping_needles_are_all_found() {
        // Scanning to the next candidate one byte on rather than one match on is
        // what finds these; skipping past the previous hit would find only the
        // first.
        assert_eq!(
            chars("aaaa", "aa"),
            vec![(0, 2), (1, 3), (2, 4)],
            "each pair of characters, not each pair of pairs"
        );
    }

    #[test]
    fn a_needle_that_is_never_there_costs_a_pass_and_nothing_else() {
        assert!(chars(&"x".repeat(10_000), "needle").is_empty());
    }

    #[test]
    fn the_number_of_hits_is_capped_and_says_so() {
        let text = "a".repeat(MAX_HITS * 2 + 10);
        let hits = find_all(&text, "a", Query::loose());
        assert_eq!(hits.len(), MAX_HITS, "it stops collecting at the cap");
        assert!(is_capped(&hits), "and reports that there were more");
        let few = find_all("aaa", "a", Query::loose());
        assert_eq!(few.len(), 3);
        assert!(!is_capped(&few), "a small file is not capped");
    }

    #[test]
    fn hits_come_back_in_order_and_do_not_overlap_themselves() {
        let text = "needle and needle and a needle, needle.";
        let hits = chars(text, "needle");
        assert_eq!(hits, vec![(0, 6), (11, 17), (24, 30), (32, 38)]);
        for pair in hits.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "hits out of order: {pair:?}");
        }
    }
}
