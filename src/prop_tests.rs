//! Randomised property tests over the pure helpers.
//!
//! The fixed tests elsewhere pin exact behaviour; these check the rules that
//! must hold for *every* input, which is where an off-by-one hides. They use
//! `proptest` and are a test-only dependency.

use proptest::prelude::*;

use crate::buffer::Buffer;
use crate::fs_model;
use crate::search;

/// The byte offset of the `chars`-th character, or the end.
fn byte_at(s: &str, chars: usize) -> usize {
    s.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn insert_str(s: &mut String, at: usize, ins: &str) {
    let b = byte_at(s, at);
    s.insert_str(b, ins);
}

fn remove_chars(s: &str, lo: usize, hi: usize) -> String {
    let (a, b) = (byte_at(s, lo), byte_at(s, hi));
    format!("{}{}", &s[..a], &s[b..])
}

fn replace_chars(s: &str, lo: usize, hi: usize, ins: &str) -> String {
    let (a, b) = (byte_at(s, lo), byte_at(s, hi));
    format!("{}{}{}", &s[..a], ins, &s[b..])
}

/// Any string of up to `n` characters, newlines and all.
fn any_text(n: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..n).prop_map(|v| v.into_iter().collect())
}

proptest! {
    /// A buffer tracks a plain `String` exactly under any run of edits, in
    /// characters. This is the invariant every hit-test and caret relies on.
    #[test]
    fn a_buffer_matches_a_string_under_random_edits(
        ops in prop::collection::vec((0u8..3, 0usize..48, "[a-z \\n]{0,6}"), 0..120)
    ) {
        let mut buf = Buffer::new();
        let mut mirror = String::new();
        for (kind, at, s) in ops {
            let n = mirror.chars().count();
            let at = at % (n + 1);
            match kind {
                0 => {
                    buf.insert(at, &s);
                    insert_str(&mut mirror, at, &s);
                }
                1 => {
                    let hi = (at + 2).min(n);
                    if at < hi {
                        buf.remove(at, hi);
                        mirror = remove_chars(&mirror, at, hi);
                    }
                }
                _ => {
                    let hi = (at + 2).min(n);
                    buf.replace(at, hi, &s);
                    mirror = replace_chars(&mirror, at, hi, &s);
                }
            }
            prop_assert_eq!(buf.to_text(), mirror.clone());
            prop_assert_eq!(buf.len_chars(), mirror.chars().count());
            prop_assert_eq!(buf.len_bytes(), mirror.len());
        }
    }

    /// The allocation-free case-insensitive search agrees with the obvious
    /// lower-case-and-contains, including for non-ASCII text.
    #[test]
    fn case_insensitive_contains_agrees_with_lowercasing(
        hay in any_text(40),
        needle in any_text(8),
    ) {
        let want = hay.to_lowercase().contains(&needle.to_lowercase());
        prop_assert_eq!(search::contains_ignore_case(&hay, &needle), want);
    }

    /// Line slices reassemble the document, and the line lookups stay ordered.
    #[test]
    fn slices_of_a_buffer_reassemble_it(text in "[a-z\\n]{0,120}") {
        let buf = Buffer::from_reader(text.as_bytes()).expect("in-memory read");
        let mut rebuilt = String::new();
        for line in 0..buf.lines() {
            let (lo, hi) = (buf.line_start(line), buf.line_end(line));
            prop_assert!(lo <= hi);
            rebuilt.push_str(&buf.slice(lo, hi));
            if line + 1 < buf.lines() {
                rebuilt.push('\n');
            }
        }
        prop_assert_eq!(rebuilt, text);
    }

    /// Verbatim-form paths are idempotent: asking twice changes nothing, and
    /// an already-absolute path comes back reaching past `MAX_PATH`.
    #[cfg(windows)]
    #[test]
    fn long_path_is_idempotent(segs in prop::collection::vec("[A-Za-z0-9_]{1,8}", 1..6)) {
        let mut p = std::path::PathBuf::from(r"C:\");
        for s in &segs {
            p.push(s);
        }
        let once = fs_model::long_path(&p);
        let twice = fs_model::long_path(&once);
        prop_assert_eq!(&once, &twice);
        prop_assert!(once.to_string_lossy().starts_with(r"\\?\"));
    }
}
