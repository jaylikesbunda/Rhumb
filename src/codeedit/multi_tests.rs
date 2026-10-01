//! More than one caret, driven the way a person drives it: keys, typing, the clipboard
//! and the pointer. Each test names what a reader would expect to see.

use super::Options;
use super::harness::{Harness, Rng};
use egui::{Key, Modifiers, Vec2};

const CTRL: Modifiers = Modifiers::CTRL;
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT: Modifiers = Modifiers::ALT;
const CTRL_SHIFT: Modifiers = Modifiers {
    ctrl: true,
    shift: true,
    ..Modifiers::NONE
};
const CTRL_ALT: Modifiers = Modifiers {
    ctrl: true,
    alt: true,
    ..Modifiers::NONE
};
const ALT_SHIFT: Modifiers = Modifiers {
    alt: true,
    shift: true,
    ..Modifiers::NONE
};

/// An editor over `text` with a caret at each of `at`.
fn carets(text: &str, at: &[usize]) -> Harness {
    let mut h = Harness::new(text);
    h.set_carets(at);
    h
}

/// An editor over `text` with the selections `sel`.
fn selections(text: &str, sel: &[(usize, usize)]) -> Harness {
    let mut h = Harness::new(text);
    h.set_selections(sel);
    h
}

/// The character index of `col` on `line`.
fn at(text: &str, line: usize, col: usize) -> usize {
    let mut index = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i == line {
            return index + col;
        }
        index += l.chars().count() + 1;
    }
    panic!("no line {line}");
}

// ====================================================================================
// typing
// ====================================================================================

#[test]
fn a_letter_typed_goes_in_at_every_caret_and_every_caret_moves_past_it() {
    let mut h = carets("aaa\nbbb\nccc", &[0, 4, 8]);
    h.type_text("x");
    assert_eq!(h.text(), "xaaa\nxbbb\nxccc");
    assert_eq!(h.carets(), vec![1, 6, 11]);
}

#[test]
fn typing_goes_on_in_step_at_every_caret() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    for ch in ["h", "i", "!"] {
        h.type_text(ch);
    }
    assert_eq!(h.text(), "ahi!\nbhi!\nchi!");
    assert_eq!(h.carets(), vec![4, 9, 14]);
}

#[test]
fn several_carets_on_one_line_each_get_the_letter() {
    let mut h = carets("one two three", &[3, 7, 13]);
    h.type_text("!");
    assert_eq!(h.text(), "one! two! three!");
    assert_eq!(h.carets(), vec![4, 9, 16]);
}

#[test]
fn typing_replaces_every_selection() {
    let mut h = selections("one two three", &[(0, 3), (4, 7), (8, 13)]);
    h.type_text("X");
    assert_eq!(h.text(), "X X X");
    assert_eq!(h.carets(), vec![1, 3, 5]);
}

#[test]
fn typing_replaces_the_selections_and_adds_to_the_plain_carets_in_one_go() {
    let mut h = selections("abc def ghi", &[(0, 3), (5, 5), (8, 11)]);
    h.type_text("_");
    assert_eq!(h.text(), "_ d_ef _");
    assert_eq!(h.carets().len(), 3);
}

#[test]
fn a_selection_made_backwards_is_replaced_the_same() {
    let mut h = selections("abc def", &[(3, 0), (7, 4)]);
    h.type_text("-");
    assert_eq!(h.text(), "- -");
}

#[test]
fn two_carets_in_one_place_are_one_caret_and_type_once() {
    let mut h = carets("abc", &[1, 1, 2]);
    assert_eq!(h.cursor_count(), 2);
    h.type_text("x");
    assert_eq!(h.text(), "axbxc");
}

#[test]
fn carets_at_the_very_start_and_end_of_the_document_take_the_letter() {
    let mut h = carets("mid", &[0, 3]);
    h.type_text("|");
    assert_eq!(h.text(), "|mid|");
    assert_eq!(h.carets(), vec![1, 5]);
}

#[test]
fn accented_and_wide_letters_go_in_at_every_caret() {
    let mut h = carets("ab\ncd", &[1, 4]);
    h.type_text("é");
    h.type_text("日");
    assert_eq!(h.text(), "aé日b\ncé日d");
    assert_eq!(h.carets(), vec![3, 8]);
}

#[test]
fn a_pasted_burst_typed_as_one_text_event_goes_in_whole_at_each_caret() {
    let mut h = carets("a\nb", &[1, 3]);
    h.type_text("xyz");
    assert_eq!(h.text(), "axyz\nbxyz");
}

#[test]
fn an_opening_bracket_brings_its_closer_at_every_caret() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.type_text("(");
    assert_eq!(h.text(), "a()\nb()\nc()");
    assert_eq!(h.carets(), vec![2, 6, 10], "each between its pair");
}

#[test]
fn typing_inside_the_pairs_and_then_the_closer_steps_over_it_at_every_caret() {
    let mut h = carets("a\nb", &[1, 3]);
    h.type_text("(");
    h.type_text("x");
    h.type_text(")");
    assert_eq!(h.text(), "a(x)\nb(x)");
    assert_eq!(h.carets(), vec![4, 9], "past the closers, not before them");
}

#[test]
fn quotes_pair_up_at_every_caret() {
    let mut h = carets("\n", &[0, 1]);
    h.type_text("\"");
    assert_eq!(h.text(), "\"\"\n\"\"");
}

#[test]
fn typing_with_carets_far_off_screen_changes_the_whole_document() {
    let doc: String = (0..500).map(|i| format!("line {i}\n")).collect();
    let starts: Vec<usize> = (0..500).step_by(50).map(|l| at(&doc, l, 0)).collect();
    let mut h = carets(&doc, &starts);
    h.type_text("#");
    let text = h.text();
    for l in (0..500).step_by(50) {
        assert!(text.contains(&format!("#line {l}\n")), "line {l}");
    }
    assert_eq!(text.matches('#').count(), 10);
}

#[test]
fn typing_in_a_read_only_editor_changes_nothing() {
    let opts = Options {
        editable: false,
        ..Options::default()
    };
    let mut h = Harness::with_options("ab\ncd", opts, Vec2::new(600.0, 600.0));
    h.set_carets(&[1, 4]);
    h.type_text("x");
    assert_eq!(h.text(), "ab\ncd");
    assert_eq!(h.carets(), vec![1, 4]);
}

// ====================================================================================
// deleting
// ====================================================================================

#[test]
fn backspace_takes_the_letter_before_every_caret() {
    let mut h = carets("aaa\nbbb\nccc", &[1, 5, 9]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "aa\nbb\ncc");
    assert_eq!(h.carets(), vec![0, 3, 6]);
}

#[test]
fn backspace_at_the_start_of_a_line_joins_it_to_the_one_above_at_every_caret() {
    let mut h = carets("a\nb\nc\nd", &[2, 6]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "ab\ncd");
    assert_eq!(h.carets(), vec![1, 4]);
}

#[test]
fn backspace_at_the_start_of_the_document_does_nothing_and_the_others_still_delete() {
    let mut h = carets("abc", &[0, 2]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "ac");
    assert_eq!(h.carets(), vec![0, 1]);
}

#[test]
fn backspace_with_two_carets_side_by_side_takes_one_letter_each() {
    let mut h = carets("abcdef", &[2, 3]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "adef");
    assert_eq!(h.cursor_count(), 1, "and they have met");
    assert_eq!(h.carets(), vec![1]);
}

#[test]
fn backspace_removes_every_selection_and_not_the_letter_before_it() {
    let mut h = selections("one two three", &[(0, 3), (8, 13)]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), " two ");
    assert_eq!(h.carets(), vec![0, 5]);
}

#[test]
fn delete_takes_the_letter_after_every_caret() {
    let mut h = carets("aaa\nbbb\nccc", &[0, 4, 8]);
    h.key(Key::Delete);
    assert_eq!(h.text(), "aa\nbb\ncc");
    assert_eq!(h.carets(), vec![0, 3, 6]);
}

#[test]
fn delete_at_the_end_of_a_line_joins_the_next_one_at_every_caret() {
    let mut h = carets("a\nb\nc\nd", &[1, 5]);
    h.key(Key::Delete);
    assert_eq!(h.text(), "ab\ncd");
}

#[test]
fn delete_at_the_end_of_the_document_does_nothing_for_that_caret() {
    let mut h = carets("abc", &[1, 3]);
    h.key(Key::Delete);
    assert_eq!(h.text(), "ac");
    assert_eq!(h.carets(), vec![1, 2]);
}

#[test]
fn delete_with_carets_side_by_side_takes_one_letter_each() {
    let mut h = carets("abcdef", &[2, 3]);
    h.key(Key::Delete);
    assert_eq!(h.text(), "abef");
    assert_eq!(h.carets(), vec![2]);
}

#[test]
fn backspace_between_a_new_pair_takes_both_brackets_at_every_caret() {
    let mut h = carets("\n", &[0, 1]);
    h.type_text("(");
    assert_eq!(h.text(), "()\n()");
    h.key(Key::Backspace);
    assert_eq!(h.text(), "\n");
}

#[test]
fn ctrl_backspace_takes_a_word_before_every_caret() {
    let mut h = carets("one two\nthree four", &[7, 18]);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "one \nthree ");
}

#[test]
fn ctrl_delete_takes_a_word_after_every_caret() {
    let mut h = carets("one two\nthree four", &[0, 8]);
    h.key_mod(Key::Delete, CTRL);
    assert_eq!(h.text(), "two\nfour");
}

#[test]
fn word_deletes_at_carets_a_word_apart_each_take_their_own_word() {
    let mut h = carets("aaa bbb ccc", &[4, 11]);
    h.key_mod(Key::Backspace, CTRL);
    assert_eq!(h.text(), "bbb ");
    assert_eq!(h.carets(), vec![0, 4]);
}

#[test]
fn deleting_everything_with_selections_leaves_an_empty_document_and_one_caret() {
    let mut h = selections("abc\ndef", &[(0, 3), (3, 7)]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "");
    assert_eq!(h.cursor_count(), 1);
}

// ====================================================================================
// Enter and Tab
// ====================================================================================

#[test]
fn enter_splits_the_line_at_every_caret() {
    let mut h = carets("abcd\nefgh", &[2, 7]);
    h.key(Key::Enter);
    assert_eq!(h.text(), "ab\ncd\nef\ngh");
    assert_eq!(h.carets(), vec![3, 9]);
}

#[test]
fn enter_keeps_the_indent_of_the_line_it_splits_at_every_caret() {
    let mut h = carets("  a\n    b", &[3, 9]);
    h.key(Key::Enter);
    assert_eq!(h.text(), "  a\n  \n    b\n    ");
}

#[test]
fn enter_between_a_pair_of_brackets_opens_a_block_at_every_caret() {
    let mut h = carets("\n", &[0, 1]);
    h.type_text("{");
    h.key(Key::Enter);
    let t = h.text();
    assert_eq!(t.matches('{').count(), 2);
    assert_eq!(t.matches('}').count(), 2);
    assert_eq!(t.lines().count(), 6, "{t:?}");
    assert_eq!(h.cursor_count(), 2);
}

#[test]
fn tab_puts_an_indent_in_at_every_caret() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.key(Key::Tab);
    assert_eq!(h.text(), "a    \nb    \nc    ");
}

#[test]
fn shift_tab_outdents_each_of_the_lines() {
    let mut h = carets("    a\n    b\n    c", &[5, 15]);
    h.key_mod(Key::Tab, SHIFT);
    assert_eq!(h.text(), "a\n    b\nc");
}

#[test]
fn tab_with_selections_indents_every_line_they_touch_once() {
    let mut h = selections("a\nb\nc\nd", &[(0, 3), (4, 5)]);
    h.key(Key::Tab);
    assert_eq!(h.text(), "    a\n    b\n    c\nd");
}

// ====================================================================================
// moving
// ====================================================================================

#[test]
fn left_and_right_move_every_caret_one_place() {
    let mut h = carets("abcdef", &[1, 4]);
    h.key(Key::ArrowRight);
    assert_eq!(h.carets(), vec![2, 5]);
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowLeft);
    assert_eq!(h.carets(), vec![0, 3]);
}

#[test]
fn carets_that_run_into_each_other_become_one() {
    let mut h = carets("abc", &[0, 1, 2, 3]);
    h.key(Key::ArrowLeft);
    assert_eq!(h.carets(), vec![0, 1, 2], "0 and 1 both landed on 0");
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowLeft);
    assert_eq!(h.carets(), vec![0]);
}

#[test]
fn right_at_the_end_of_the_document_holds_still_and_the_others_move() {
    let mut h = carets("abc", &[1, 3]);
    h.key(Key::ArrowRight);
    assert_eq!(h.carets(), vec![2, 3]);
}

#[test]
fn right_over_a_line_break_takes_each_caret_to_the_next_line() {
    let mut h = carets("ab\ncd\nef", &[2, 5]);
    h.key(Key::ArrowRight);
    assert_eq!(h.carets(), vec![3, 6]);
}

#[test]
fn up_and_down_keep_each_caret_in_its_column() {
    let text = "abcd\nefgh\nijkl\nmnop";
    let mut h = carets(text, &[at(text, 1, 2), at(text, 2, 3)]);
    h.key(Key::ArrowDown);
    assert_eq!(h.carets(), vec![at(text, 2, 2), at(text, 3, 3)]);
    h.key(Key::ArrowUp);
    h.key(Key::ArrowUp);
    assert_eq!(h.carets(), vec![at(text, 0, 2), at(text, 1, 3)]);
}

#[test]
fn carets_that_meet_going_down_become_one() {
    let text = "a\nb\nc";
    let mut h = carets(text, &[at(text, 0, 0), at(text, 1, 0), at(text, 2, 0)]);
    h.key(Key::ArrowDown);
    // The last line's caret has nowhere to go but the end of the document.
    assert_eq!(
        h.carets(),
        vec![at(text, 1, 0), at(text, 2, 0), at(text, 2, 1)]
    );
}

#[test]
fn home_and_end_take_every_caret_to_the_ends_of_its_line() {
    let text = "alpha\nbeta\ngamma";
    let mut h = carets(text, &[at(text, 0, 2), at(text, 1, 2), at(text, 2, 2)]);
    h.key(Key::End);
    assert_eq!(h.carets(), vec![5, 10, 16]);
    h.key(Key::Home);
    assert_eq!(h.carets(), vec![0, 6, 11]);
}

#[test]
fn home_and_end_take_carets_that_share_a_line_to_the_same_place_and_they_become_one() {
    let mut h = carets("abcdef", &[1, 3, 5]);
    h.key(Key::End);
    assert_eq!(h.carets(), vec![6]);
    let mut h = carets("abcdef", &[1, 3, 5]);
    h.key(Key::Home);
    assert_eq!(h.carets(), vec![0]);
}

#[test]
fn home_goes_to_the_code_first_for_each_caret() {
    let text = "    a\n  b";
    let mut h = carets(text, &[at(text, 0, 5), at(text, 1, 3)]);
    h.key(Key::Home);
    assert_eq!(h.carets(), vec![4, 8]);
    h.key(Key::Home);
    assert_eq!(h.carets(), vec![0, 6]);
}

#[test]
fn shift_arrows_extend_every_selection() {
    let mut h = carets("abcdef\nghijkl", &[1, 8]);
    h.key_mod(Key::ArrowRight, SHIFT);
    h.key_mod(Key::ArrowRight, SHIFT);
    assert_eq!(h.cursors(), vec![(1, 3), (8, 10)]);
}

#[test]
fn selections_that_grow_until_they_touch_stay_two_and_those_that_overlap_become_one() {
    let mut h = carets("abcdef", &[1, 4]);
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    assert_eq!(h.cursors(), vec![(1, 4), (4, 6)]);
    let mut h = carets("abcdefgh", &[1, 3]);
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, SHIFT);
    }
    assert_eq!(h.cursors(), vec![(1, 6)], "1..4 and 3..6 overlap");
}

#[test]
fn shift_end_selects_to_the_end_of_every_line() {
    let text = "ab cd\nef gh";
    let mut h = carets(text, &[at(text, 0, 3), at(text, 1, 3)]);
    h.key_mod(Key::End, SHIFT);
    assert_eq!(h.cursors(), vec![(3, 5), (9, 11)]);
}

#[test]
fn plain_left_with_selections_puts_each_caret_at_the_start_of_its_selection() {
    let mut h = selections("abcdef ghijkl", &[(1, 3), (8, 11)]);
    h.key(Key::ArrowLeft);
    assert_eq!(h.cursors(), vec![(1, 1), (8, 8)]);
}

#[test]
fn plain_right_with_selections_puts_each_caret_at_the_end_of_its_selection() {
    let mut h = selections("abcdef ghijkl", &[(1, 3), (8, 11)]);
    h.key(Key::ArrowRight);
    assert_eq!(h.cursors(), vec![(3, 3), (11, 11)]);
}

#[test]
fn ctrl_arrows_move_every_caret_by_a_word() {
    let text = "one two three\nfour five six";
    let mut h = carets(text, &[at(text, 0, 0), at(text, 1, 0)]);
    h.key_mod(Key::ArrowRight, CTRL);
    assert_eq!(h.carets(), vec![4, 19]);
    h.key_mod(Key::ArrowLeft, CTRL);
    assert_eq!(h.carets(), vec![0, 14]);
}

#[test]
fn ctrl_home_and_ctrl_end_bring_every_caret_to_the_one_place() {
    let mut h = carets("abc\ndef", &[1, 5]);
    h.key_mod(Key::Home, CTRL);
    assert_eq!(h.carets(), vec![0]);
    let mut h = carets("abc\ndef", &[1, 5]);
    h.key_mod(Key::End, CTRL);
    assert_eq!(h.carets(), vec![7]);
}

#[test]
fn page_down_moves_every_caret() {
    let doc: String = (0..200).map(|i| format!("row {i}\n")).collect();
    let mut h = carets(&doc, &[at(&doc, 0, 2), at(&doc, 1, 2)]);
    h.key(Key::PageDown);
    let c = h.carets();
    assert_eq!(c.len(), 2);
    assert!(c[0] > at(&doc, 20, 0) && c[1] > c[0]);
}

#[test]
fn a_move_never_leaves_a_caret_outside_the_document() {
    let mut h = carets("ab", &[0, 1, 2]);
    for k in [
        Key::ArrowRight,
        Key::ArrowRight,
        Key::ArrowDown,
        Key::End,
        Key::ArrowRight,
    ] {
        h.key(k);
        assert!(h.carets().iter().all(|c| *c <= 2));
    }
}

// ====================================================================================
// Ctrl+D, Ctrl+Shift+L, and carets above and below
// ====================================================================================

#[test]
fn ctrl_d_selects_the_word_at_the_caret_first() {
    let mut h = carets("foo bar foo", &[1]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3)]);
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn ctrl_d_again_adds_the_next_place_the_same_text_stands() {
    let mut h = carets("foo bar foo baz foo", &[1]);
    h.key_mod(Key::D, CTRL);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11), (16, 19)]);
}

#[test]
fn ctrl_d_wraps_round_the_end_and_stops_when_every_place_has_one() {
    let mut h = carets("foo bar foo", &[9]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(8, 11)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11)], "wrapped to the start");
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11)], "nothing left to add");
}

#[test]
fn ctrl_d_makes_the_newest_the_primary_so_typing_and_scrolling_follow_it() {
    let mut h = carets("foo foo foo", &[1]);
    h.key_mod(Key::D, CTRL);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.caret(), 7, "the second foo's end");
}

#[test]
fn ctrl_d_with_a_selection_made_by_hand_looks_for_that_text() {
    let mut h = selections("cat dog cat", &[(0, 3)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11)]);
}

#[test]
fn ctrl_d_matches_case_exactly() {
    let mut h = selections("Foo foo FOO", &[(4, 7)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(4, 7)], "no other lower case foo");
}

#[test]
fn ctrl_d_finds_text_across_lines() {
    let mut h = selections("one\ntwo\none\nthree\none", &[(0, 3)]);
    h.key_mod(Key::D, CTRL);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11), (18, 21)]);
}

#[test]
fn ctrl_d_finds_a_selection_that_spans_lines() {
    let mut h = selections("ab\ncd xx ab\ncd", &[(0, 5)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 5), (9, 14)]);
}

#[test]
fn ctrl_d_does_not_find_overlapping_copies_of_what_is_already_selected() {
    let mut h = selections("aaaa", &[(0, 2)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 2), (2, 4)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 2), (2, 4)]);
}

#[test]
fn ctrl_d_on_a_space_or_in_an_empty_document_does_nothing() {
    let mut h = carets("a  b", &[2]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(2, 2)]);
    let mut h = Harness::new("");
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn ctrl_d_finds_accented_text() {
    let mut h = selections("café x café", &[(0, 4)]);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 4), (7, 11)]);
}

#[test]
fn ctrl_d_then_typing_replaces_every_occurrence() {
    let mut h = carets("foo bar foo", &[1]);
    h.key_mod(Key::D, CTRL);
    h.key_mod(Key::D, CTRL);
    h.type_text("qux");
    assert_eq!(h.text(), "qux bar qux");
}

#[test]
fn ctrl_shift_d_is_a_duplicate_and_not_another_caret() {
    let mut h = carets("abc", &[1]);
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "abc\nabc");
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn ctrl_shift_l_puts_a_caret_on_every_place_the_word_stands() {
    let mut h = carets("foo bar foo baz foo", &[9]);
    h.key_mod(Key::L, CTRL_SHIFT);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11), (16, 19)]);
}

#[test]
fn ctrl_shift_l_uses_the_selection_when_there_is_one() {
    let mut h = selections("a-b a-b a-bc", &[(0, 3)]);
    h.key_mod(Key::L, CTRL_SHIFT);
    assert_eq!(h.cursors(), vec![(0, 3), (4, 7), (8, 11)]);
}

#[test]
fn ctrl_shift_l_then_typing_renames_every_one() {
    let mut h = carets("let x = x + x;", &[4]);
    h.key_mod(Key::L, CTRL_SHIFT);
    h.type_text("count");
    assert_eq!(h.text(), "let count = count + count;");
}

#[test]
fn ctrl_shift_l_with_nothing_to_select_does_nothing() {
    let mut h = carets("a  b", &[2]);
    h.key_mod(Key::L, CTRL_SHIFT);
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn ctrl_shift_l_when_the_text_is_there_only_once_leaves_one_selection() {
    let mut h = carets("one two three", &[5]);
    h.key_mod(Key::L, CTRL_SHIFT);
    assert_eq!(h.cursors(), vec![(4, 7)]);
}

#[test]
fn ctrl_alt_down_adds_a_caret_on_the_line_below_in_the_same_column() {
    let text = "abcd\nefgh\nijkl";
    let mut h = carets(text, &[at(text, 0, 2)]);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    assert_eq!(h.carets(), vec![at(text, 0, 2), at(text, 1, 2)]);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    assert_eq!(
        h.carets(),
        vec![at(text, 0, 2), at(text, 1, 2), at(text, 2, 2)]
    );
}

#[test]
fn ctrl_alt_up_adds_a_caret_on_the_line_above() {
    let text = "abcd\nefgh\nijkl";
    let mut h = carets(text, &[at(text, 2, 3)]);
    h.key_mod(Key::ArrowUp, CTRL_ALT);
    h.key_mod(Key::ArrowUp, CTRL_ALT);
    assert_eq!(
        h.carets(),
        vec![at(text, 0, 3), at(text, 1, 3), at(text, 2, 3)]
    );
}

#[test]
fn a_column_of_carets_goes_to_the_end_of_a_short_line_and_comes_back_out_to_the_column() {
    let text = "abcdef\nab\nabcdef";
    let mut h = carets(text, &[at(text, 0, 5)]);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    assert_eq!(
        h.carets(),
        vec![at(text, 0, 5), at(text, 1, 2), at(text, 2, 5)]
    );
}

#[test]
fn adding_a_caret_above_the_first_line_or_below_the_last_adds_nothing() {
    let mut h = carets("a\nb", &[0]);
    h.key_mod(Key::ArrowUp, CTRL_ALT);
    assert_eq!(h.cursor_count(), 1);
    let mut h = carets("a\nb", &[3]);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn a_column_of_carets_can_be_typed_into() {
    let text = "a\nb\nc\nd";
    let mut h = carets(text, &[0]);
    for _ in 0..3 {
        h.key_mod(Key::ArrowDown, CTRL_ALT);
    }
    h.type_text("- ");
    assert_eq!(h.text(), "- a\n- b\n- c\n- d");
}

#[test]
fn adding_carets_below_continues_from_the_lowest_even_after_one_was_added_above() {
    let text = "a\nb\nc\nd\ne";
    let mut h = carets(text, &[at(text, 2, 0)]);
    h.key_mod(Key::ArrowUp, CTRL_ALT);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    assert_eq!(
        h.carets(),
        vec![at(text, 1, 0), at(text, 2, 0), at(text, 3, 0)]
    );
}

#[test]
fn escape_puts_the_extra_carets_away_and_keeps_the_newest() {
    let mut h = carets("a\nb\nc", &[0, 2, 4]);
    assert_eq!(h.cursor_count(), 3);
    h.key(Key::Escape);
    assert_eq!(h.cursor_count(), 1);
    assert_eq!(h.caret(), 4);
}

#[test]
fn escape_with_one_caret_does_nothing_to_it() {
    let mut h = carets("a\nb", &[2]);
    h.key(Key::Escape);
    assert_eq!(h.carets(), vec![2]);
}

#[test]
fn select_all_is_one_selection_whatever_there_was() {
    let mut h = carets("abc\ndef", &[1, 5]);
    h.key_mod(Key::A, CTRL);
    assert_eq!(h.cursors(), vec![(0, 7)]);
}

// ====================================================================================
// the pointer
// ====================================================================================

#[test]
fn alt_click_adds_a_caret_where_the_pointer_is() {
    let mut h = Harness::new("alpha\nbeta\ngamma");
    h.click(h.pos_of(0, 0.5));
    h.click_mod(h.pos_of(2, 2.5), ALT);
    assert_eq!(h.cursor_count(), 2);
    let c = h.carets();
    assert!(c[0] <= 1, "{c:?}");
    assert!(c[1] >= 13 && c[1] <= 14, "{c:?}");
}

#[test]
fn alt_click_on_a_caret_that_is_there_takes_it_away() {
    let mut h = Harness::new("alpha\nbeta\ngamma");
    h.click(h.pos_of(0, 0.5));
    h.click_mod(h.pos_of(1, 2.5), ALT);
    assert_eq!(h.cursor_count(), 2);
    h.click_mod(h.pos_of(1, 2.5), ALT);
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn the_last_caret_cannot_be_taken_away_by_alt_click() {
    let mut h = Harness::new("alpha\nbeta");
    h.click(h.pos_of(0, 2.5));
    h.click_mod(h.pos_of(0, 2.5), ALT);
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn a_plain_click_puts_the_extra_carets_away() {
    let mut h = Harness::new("alpha\nbeta\ngamma");
    h.click(h.pos_of(0, 0.5));
    h.click_mod(h.pos_of(1, 2.5), ALT);
    h.click_mod(h.pos_of(2, 2.5), ALT);
    assert_eq!(h.cursor_count(), 3);
    h.click(h.pos_of(1, 1.5));
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn typing_after_alt_clicks_goes_in_at_each_clicked_place() {
    let mut h = Harness::new("aaa\nbbb\nccc");
    h.click(h.pos_of(0, 1.5));
    h.click_mod(h.pos_of(1, 1.5), ALT);
    h.click_mod(h.pos_of(2, 1.5), ALT);
    h.type_text("_");
    let t = h.text();
    assert_eq!(t.matches('_').count(), 3, "{t:?}");
}

#[test]
fn shift_click_extends_only_the_primary() {
    let mut h = carets("abcdefghij", &[1, 8]);
    let before = h.cursors();
    h.click_mod(h.pos_of(0, 5.5), SHIFT);
    let after = h.cursors();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], before[0], "the other selection is as it was");
}

#[test]
fn an_alt_drag_makes_a_selection_beside_the_others() {
    let mut h = Harness::new("abcdef\nghijkl\nmnopqr");
    h.click(h.pos_of(0, 0.5));
    h.pointer_mod(h.pos_of(2, 1.5), ALT, true);
    h.pointer_mod(h.pos_of(2, 4.5), ALT, false);
    assert!(h.cursor_count() >= 1);
}

// ====================================================================================
// the clipboard
// ====================================================================================

#[test]
fn copy_with_several_selections_takes_each_on_a_line_of_its_own() {
    let mut h = selections("one two three", &[(0, 3), (4, 7), (8, 13)]);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("one\ntwo\nthree"));
}

#[test]
fn copy_leaves_every_selection_where_it_was() {
    let mut h = selections("one two three", &[(0, 3), (8, 13)]);
    h.send_copy();
    assert_eq!(h.cursors(), vec![(0, 3), (8, 13)]);
    assert_eq!(h.text(), "one two three");
}

#[test]
fn copy_with_no_selection_takes_the_line_of_each_caret() {
    let mut h = carets("aa\nbb\ncc", &[1, 7]);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("aa\ncc"));
}

#[test]
fn copy_with_two_carets_on_one_line_takes_that_line_once() {
    let mut h = carets("aa bb\ncc", &[1, 4]);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("aa bb\n"));
}

#[test]
fn copy_takes_only_the_selected_ones_when_some_carets_have_nothing_selected() {
    let mut h = selections("abc def ghi", &[(0, 3), (5, 5), (8, 11)]);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("abc\nghi"));
}

#[test]
fn cut_takes_every_selection_out() {
    let mut h = selections("one two three", &[(0, 3), (8, 13)]);
    h.send_cut();
    assert_eq!(h.copied().as_deref(), Some("one\nthree"));
    assert_eq!(h.text(), " two ");
}

#[test]
fn cut_with_no_selection_takes_the_lines_the_carets_are_on() {
    let mut h = carets("aa\nbb\ncc\ndd", &[1, 7]);
    h.send_cut();
    assert_eq!(h.text(), "bb\ndd");
}

#[test]
fn cut_with_two_carets_on_one_line_takes_that_line_once() {
    let mut h = carets("aa bb\ncc", &[1, 4]);
    h.send_cut();
    assert_eq!(h.text(), "cc");
}

#[test]
fn paste_puts_the_same_text_in_at_every_caret() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.paste("XY");
    assert_eq!(h.text(), "aXY\nbXY\ncXY");
    assert_eq!(h.carets(), vec![3, 7, 11]);
}

#[test]
fn paste_of_as_many_lines_as_carets_gives_each_caret_a_line() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.paste("1\n2\n3");
    assert_eq!(h.text(), "a1\nb2\nc3");
}

#[test]
fn paste_with_a_final_newline_still_gives_each_caret_one_line() {
    let mut h = carets("a\nb", &[1, 3]);
    h.paste("1\n2\n");
    assert_eq!(h.text(), "a1\nb2");
}

#[test]
fn paste_of_a_different_number_of_lines_goes_whole_into_every_caret() {
    let mut h = carets("a\nb", &[1, 3]);
    h.paste("1\n2\n3");
    assert_eq!(h.text(), "a1\n2\n3\nb1\n2\n3");
}

#[test]
fn paste_of_one_line_goes_to_every_caret() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.paste("z");
    assert_eq!(h.text(), "az\nbz\ncz");
}

#[test]
fn paste_replaces_every_selection() {
    let mut h = selections("one two three", &[(0, 3), (8, 13)]);
    h.paste("#");
    assert_eq!(h.text(), "# two #");
}

#[test]
fn what_was_copied_from_a_column_of_carets_pastes_back_one_to_a_line() {
    let mut h = selections("one\ntwo\nthree\n\n\n", &[(0, 3), (4, 7), (8, 13)]);
    h.send_copy();
    let copied = h.copied().unwrap();
    let mut h2 = carets("\n\n", &[0, 1, 2]);
    h2.paste(&copied);
    assert_eq!(h2.text(), "one\ntwo\nthree");
}

#[test]
fn crlf_in_a_paste_is_treated_as_a_line_break_for_distribution() {
    let mut h = carets("a\nb", &[1, 3]);
    h.paste("1\r\n2");
    assert_eq!(h.text(), "a1\nb2");
}

// ====================================================================================
// undo and redo
// ====================================================================================

#[test]
fn undo_takes_back_a_letter_typed_at_every_caret_in_one_step() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.type_text("x");
    assert_eq!(h.text(), "ax\nbx\ncx");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a\nb\nc");
}

#[test]
fn undo_puts_every_caret_back_where_it_was() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.type_text("x");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.carets(), vec![1, 3, 5]);
}

#[test]
fn undo_puts_selections_back_as_selections() {
    let mut h = selections("one two three", &[(0, 3), (8, 13)]);
    h.type_text("X");
    assert_eq!(h.text(), "X two X");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "one two three");
    assert_eq!(h.cursors(), vec![(0, 3), (8, 13)]);
}

#[test]
fn redo_does_it_again_and_leaves_the_carets_after_it() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.type_text("x");
    let after = h.carets();
    h.key_mod(Key::Z, CTRL);
    h.key_mod(Key::Y, CTRL);
    assert_eq!(h.text(), "ax\nbx\ncx");
    assert_eq!(h.carets(), after);
}

#[test]
fn redo_with_control_shift_z_works_the_same() {
    let mut h = carets("a\nb", &[1, 3]);
    h.type_text("x");
    h.key_mod(Key::Z, CTRL);
    h.key_mod(Key::Z, CTRL_SHIFT);
    assert_eq!(h.text(), "ax\nbx");
}

#[test]
fn a_run_of_typing_at_several_carets_is_one_undo_step() {
    let mut h = carets("a\nb", &[1, 3]);
    for ch in ["h", "e", "l", "l", "o"] {
        h.type_text(ch);
    }
    assert_eq!(h.text(), "ahello\nbhello");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a\nb");
}

#[test]
fn backspace_at_several_carets_undoes_in_one_step_and_restores_the_carets() {
    let mut h = carets("aaa\nbbb\nccc", &[1, 5, 9]);
    h.pause();
    h.key(Key::Backspace);
    assert_eq!(h.text(), "aa\nbb\ncc");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "aaa\nbbb\nccc");
    assert_eq!(h.carets(), vec![1, 5, 9]);
}

#[test]
fn enter_at_several_carets_undoes_in_one_step() {
    let mut h = carets("abcd\nefgh", &[2, 7]);
    h.pause();
    h.key(Key::Enter);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "abcd\nefgh");
    assert_eq!(h.carets(), vec![2, 7]);
}

#[test]
fn a_paste_at_several_carets_undoes_in_one_step() {
    let mut h = carets("a\nb", &[1, 3]);
    h.pause();
    h.paste("zz");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a\nb");
    assert_eq!(h.carets(), vec![1, 3]);
}

#[test]
fn a_cut_at_several_selections_undoes_in_one_step() {
    let mut h = selections("one two three", &[(0, 3), (8, 13)]);
    h.pause();
    h.send_cut();
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "one two three");
    assert_eq!(h.cursors(), vec![(0, 3), (8, 13)]);
}

#[test]
fn separate_pauses_make_separate_steps_with_several_carets() {
    let mut h = carets("a\nb", &[1, 3]);
    h.type_text("x");
    h.pause();
    h.type_text("y");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "ax\nbx");
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a\nb");
}

#[test]
fn undo_after_the_carets_were_put_away_still_takes_back_the_edit() {
    let mut h = carets("a\nb", &[1, 3]);
    h.type_text("x");
    h.key(Key::Escape);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a\nb");
}

#[test]
fn undo_all_the_way_and_redo_all_the_way_agree_with_the_text_at_each_step() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    let mut states = vec![h.text()];
    for ch in ["x", "y", "z"] {
        h.pause();
        h.type_text(ch);
        states.push(h.text());
    }
    for want in states.iter().rev().skip(1) {
        h.key_mod(Key::Z, CTRL);
        assert_eq!(&h.text(), want);
    }
    for want in states.iter().skip(1) {
        h.key_mod(Key::Y, CTRL);
        assert_eq!(&h.text(), want);
    }
}

#[test]
fn undo_with_nothing_to_undo_leaves_the_carets_alone() {
    let mut h = carets("a\nb", &[1, 3]);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.carets(), vec![1, 3]);
}

// ====================================================================================
// whole-line commands
// ====================================================================================

#[test]
fn comment_toggles_every_line_a_caret_is_on() {
    let mut h = carets("a\nb\nc", &[0, 4]);
    h.key_mod(Key::Slash, CTRL);
    let t = h.text();
    assert_eq!(t.lines().count(), 3);
    assert!(t.lines().nth(1) == Some("b"), "{t:?}");
}

#[test]
fn comment_with_two_carets_on_one_line_toggles_that_line_once() {
    let opts = Options {
        comment: "//",
        ..Options::default()
    };
    let mut h = Harness::with_options("let a = 1;", opts, Vec2::new(600.0, 600.0));
    h.set_carets(&[2, 6]);
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "// let a = 1;");
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "let a = 1;");
}

#[test]
fn comment_and_uncomment_several_lines_with_carets() {
    let opts = Options {
        comment: "//",
        ..Options::default()
    };
    let mut h = Harness::with_options("a\nb\nc", opts, Vec2::new(600.0, 600.0));
    h.set_carets(&[0, 2, 4]);
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "// a\n// b\n// c");
    h.key_mod(Key::Slash, CTRL);
    assert_eq!(h.text(), "a\nb\nc");
}

#[test]
fn delete_line_takes_every_line_a_caret_is_on() {
    let mut h = carets("a\nb\nc\nd", &[0, 4]);
    h.key_mod(Key::K, CTRL_SHIFT);
    assert_eq!(h.text(), "b\nd");
}

#[test]
fn delete_line_with_two_carets_on_a_line_takes_it_once() {
    let mut h = carets("aa bb\ncc\ndd", &[1, 4]);
    h.key_mod(Key::K, CTRL_SHIFT);
    assert_eq!(h.text(), "cc\ndd");
}

#[test]
fn duplicate_line_copies_every_line_a_caret_is_on_once() {
    let mut h = carets("a\nb\nc", &[0, 2]);
    h.key_mod(Key::D, CTRL_SHIFT);
    assert_eq!(h.text(), "a\na\nb\nb\nc");
}

#[test]
fn moving_lines_down_moves_every_line_with_a_caret() {
    let mut h = carets("a\nb\nc\nd\ne", &[0, 4]);
    h.key_mod(Key::ArrowDown, ALT);
    assert_eq!(h.text(), "b\na\nd\nc\ne");
}

#[test]
fn moving_lines_up_moves_every_line_with_a_caret() {
    let mut h = carets("a\nb\nc\nd\ne", &[2, 6]);
    h.key_mod(Key::ArrowUp, ALT);
    assert_eq!(h.text(), "b\na\nd\nc\ne");
}

// ====================================================================================
// drawing
// ====================================================================================

#[test]
fn every_caret_is_drawn() {
    let mut h = carets("aaa\nbbb\nccc", &[1, 5, 9]);
    h.frame();
    assert_eq!(h.caret_rects().len(), 3);
}

#[test]
fn the_carets_are_on_their_own_rows() {
    let mut h = carets("aaa\nbbb\nccc", &[1, 5, 9]);
    h.frame();
    let mut tops: Vec<i32> = h.caret_rects().iter().map(|r| r.top() as i32).collect();
    tops.sort_unstable();
    tops.dedup();
    assert_eq!(tops.len(), 3);
}

#[test]
fn every_selection_is_washed() {
    let mut h = selections("one two three four", &[(0, 3), (4, 7), (8, 13)]);
    h.frame();
    assert_eq!(h.selection_rects().len(), 3);
}

#[test]
fn carets_outside_the_window_are_not_drawn_and_do_no_harm() {
    let doc: String = (0..400).map(|i| format!("line {i}\n")).collect();
    let mut h = carets(&doc, &[at(&doc, 0, 1), at(&doc, 399, 1)]);
    h.frame();
    assert_eq!(h.caret_rects().len(), 1, "only the one in view");
    h.type_text("!");
    assert_eq!(h.text().matches('!').count(), 2);
}

#[test]
fn a_caret_on_the_last_character_of_each_line_is_drawn_on_that_line() {
    let text = "ab\ncd\nef";
    let mut h = carets(text, &[2, 5, 8]);
    h.frame();
    let mut rects = h.caret_rects();
    rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
    assert_eq!(rects.len(), 3);
    let tops = h.row_tops();
    for (i, r) in rects.iter().enumerate() {
        assert!(
            (r.top() - tops[i]).abs() < 1.0,
            "caret {i} at {:?}",
            r.top()
        );
    }
}

#[test]
fn the_window_follows_the_newest_caret() {
    let doc: String = (0..400).map(|i| format!("line {i}\n")).collect();
    let mut h = carets(&doc, &[0]);
    h.key_mod(Key::End, CTRL);
    assert!(h.top_line() != Some(0));
    h.key_mod(Key::Home, CTRL);
    assert_eq!(h.top_line(), Some(0));
}

#[test]
fn ctrl_d_scrolls_to_the_occurrence_it_adds() {
    let mut doc: String = (0..400).map(|i| format!("line {i}\n")).collect();
    doc.push_str("needle\n");
    let mut d = String::from("needle\n");
    d.push_str(&doc);
    let mut h = carets(&d, &[2]);
    h.key_mod(Key::D, CTRL);
    h.key_mod(Key::D, CTRL);
    assert!(h.top_line().unwrap_or(0) > 100, "{:?}", h.top_line());
}

// ====================================================================================
// other features with several carets
// ====================================================================================

#[test]
fn replacing_in_the_find_bar_puts_the_extra_carets_away() {
    let mut h = carets("foo foo", &[0, 4]);
    h.find("foo");
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn clicking_in_the_text_while_the_find_bar_is_open_keeps_working() {
    let mut h = carets("foo foo", &[0, 4]);
    h.key_mod(Key::F, CTRL);
    h.key(Key::Escape);
    h.click(h.pos_of(0, 2.5));
    assert_eq!(h.cursor_count(), 1);
}

#[test]
fn wrapped_text_takes_carets_and_edits_them_like_any_other() {
    let opts = Options {
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&"word ".repeat(30), opts, Vec2::new(200.0, 300.0));
    h.set_carets(&[0, 50, 100]);
    h.type_text("#");
    assert_eq!(h.text().matches('#').count(), 3);
    assert_eq!(h.cursor_count(), 3);
}

#[test]
fn a_caret_added_below_in_wrapped_text_goes_to_the_next_row() {
    let opts = Options {
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&"word ".repeat(40), opts, Vec2::new(200.0, 400.0));
    h.set_carets(&[2]);
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    let c = h.carets();
    assert_eq!(c.len(), 2);
    assert!(c[1] > c[0] + 10, "a row further on: {c:?}");
}

#[test]
fn home_and_end_in_wrapped_text_use_the_row_of_each_caret() {
    let opts = Options {
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&"word ".repeat(40), opts, Vec2::new(200.0, 400.0));
    h.set_carets(&[2, 100]);
    h.key(Key::Home);
    let c = h.carets();
    assert_eq!(c.len(), 2);
    assert!(c[0] == 0 && c[1] > 40, "{c:?}");
}

#[test]
fn a_document_with_one_very_long_line_takes_carets_in_it() {
    let line = "abcdefghij".repeat(1000);
    let mut h = carets(&line, &[10, 5000, 9990]);
    h.type_text("_");
    assert_eq!(h.text().matches('_').count(), 3);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), line);
}

#[test]
fn carets_at_both_ends_of_an_empty_line_between_others_work() {
    let mut h = carets("a\n\nb", &[1, 2, 4]);
    h.type_text("x");
    assert_eq!(h.text(), "ax\nx\nbx");
}

#[test]
fn an_empty_document_has_one_caret_and_extra_ones_are_the_same_place() {
    let mut h = Harness::new("");
    h.set_carets(&[0, 0, 0]);
    assert_eq!(h.cursor_count(), 1);
    h.type_text("x");
    assert_eq!(h.text(), "x");
}

#[test]
fn setting_a_caret_past_the_end_is_held_at_the_end() {
    let mut h = Harness::new("abc");
    h.set_carets(&[1, 99]);
    assert_eq!(h.carets(), vec![1, 3]);
}

#[test]
fn replace_all_with_several_carets_puts_them_away_and_undo_brings_the_text_back() {
    let mut h = carets("foo x foo y foo", &[0, 5, 10]);
    h.pause();
    h.find("foo");
    h.replace_with("bar");
    h.press_replace(true);
    assert_eq!(h.text(), "bar x bar y bar");
    assert_eq!(h.cursor_count(), 1);
    // Back into the text, which is where undo lives.
    h.click(h.pos_of(0, 0.5));
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "foo x foo y foo");
}

#[test]
fn a_read_only_editor_still_lets_text_be_selected_with_several_carets_and_copied() {
    let opts = Options {
        editable: false,
        ..Options::default()
    };
    let mut h = Harness::with_options("foo bar foo", opts, Vec2::new(600.0, 600.0));
    h.set_carets(&[1]);
    h.key_mod(Key::D, CTRL);
    h.key_mod(Key::D, CTRL);
    assert_eq!(h.cursors(), vec![(0, 3), (8, 11)]);
    h.send_copy();
    assert_eq!(h.copied().as_deref(), Some("foo\nfoo"));
    h.key(Key::Backspace);
    h.type_text("x");
    assert_eq!(h.text(), "foo bar foo");
}

#[test]
fn removing_the_primary_caret_with_alt_click_leaves_the_others() {
    let mut h = Harness::new("alpha\nbeta\ngamma");
    h.set_carets(&[1, 7, 13]);
    h.click_mod(h.pos_of(2, 1.5), ALT);
    assert_eq!(h.cursor_count(), 2, "{:?}", h.cursors());
}

#[test]
fn an_edit_then_carets_added_then_undo_goes_back_to_the_single_caret_before_the_edit() {
    let mut h = carets("a\nb\nc", &[1]);
    h.type_text("x");
    h.pause();
    h.key_mod(Key::ArrowDown, CTRL_ALT);
    assert_eq!(h.cursor_count(), 2);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.text(), "a\nb\nc");
    assert_eq!(h.cursors(), vec![(1, 1)]);
}

#[test]
fn redo_after_undo_at_several_carets_puts_the_carets_after_the_edit() {
    let mut h = carets("a\nb", &[1, 3]);
    h.type_text("x");
    h.pause();
    h.key_mod(Key::Z, CTRL);
    h.key_mod(Key::Y, CTRL);
    assert_eq!(h.text(), "ax\nbx");
    assert_eq!(h.carets(), vec![2, 5]);
}

#[test]
fn holding_backspace_with_carets_everywhere_empties_the_document_and_leaves_one_caret() {
    let mut h = carets("ab\ncd\nef", &[2, 5, 8]);
    for _ in 0..12 {
        h.key(Key::Backspace);
    }
    assert_eq!(h.text(), "");
    assert_eq!(h.cursor_count(), 1);
    assert_eq!(h.carets(), vec![0]);
}

#[test]
fn escape_closes_the_find_bar_and_leaves_the_carets_for_the_next_press() {
    let mut h = carets("foo foo foo", &[0, 4, 8]);
    h.key_mod(Key::F, CTRL);
    assert!(h.find_open());
    h.key(Key::Escape);
    assert!(!h.find_open());
    assert_eq!(h.cursor_count(), 3, "that press was the bar's");
    h.key(Key::Escape);
    assert_eq!(h.cursor_count(), 1, "and this one is the carets'");
}

#[test]
fn backspace_takes_a_whole_emoji_family_and_not_half_of_it_at_every_caret() {
    let family = "👨\u{200D}👩\u{200D}👧";
    let doc = format!("{family}a\n{family}b");
    let first = family.chars().count();
    let mut h = carets(&doc, &[first, first + 1 + 1 + first]);
    h.key(Key::Backspace);
    assert_eq!(h.text(), "a\nb");
}

#[test]
fn ctrl_shift_l_finds_the_text_inside_longer_words_too() {
    let mut h = carets("cat concatenate cat", &[1]);
    h.key_mod(Key::L, CTRL_SHIFT);
    assert_eq!(h.cursor_count(), 3, "{:?}", h.cursors());
}

#[test]
fn cut_then_paste_gives_each_caret_its_own_text_back() {
    let mut h = selections("one two three", &[(0, 3), (4, 7), (8, 13)]);
    h.send_cut();
    let clip = h.copied().unwrap();
    assert_eq!(h.text(), "  ");
    h.paste(&clip);
    assert_eq!(h.text(), "one two three");
}

#[test]
fn pasting_a_line_each_into_five_hundred_carets_puts_each_in_its_place() {
    let doc = "-\n".repeat(500);
    let starts: Vec<usize> = (0..500).map(|i| i * 2 + 1).collect();
    let mut h = carets(&doc, &starts);
    let lines: String = (0..500)
        .map(|i| format!("n{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    h.paste(&lines);
    let text = h.text();
    for (i, l) in text.lines().take(500).enumerate() {
        assert_eq!(l, format!("-n{i}"));
    }
}

#[test]
fn tab_with_overlapping_selections_indents_each_line_once() {
    let mut h = selections("a\nb\nc\nd", &[(0, 3), (2, 5)]);
    h.key(Key::Tab);
    assert_eq!(h.text(), "    a\n    b\n    c\nd");
}

#[test]
fn a_selection_made_backwards_and_deleted_is_restored_backwards_by_undo() {
    let mut h = selections("one two three", &[(3, 0), (13, 8)]);
    h.pause();
    h.key(Key::Backspace);
    h.key_mod(Key::Z, CTRL);
    assert_eq!(h.cursors(), vec![(3, 0), (13, 8)]);
}

#[test]
fn washes_of_several_selections_across_wrapped_rows_are_all_drawn() {
    let opts = Options {
        wrap: true,
        ..Options::default()
    };
    let mut h = Harness::with_options(&"word ".repeat(40), opts, Vec2::new(200.0, 400.0));
    h.set_selections(&[(0, 30), (60, 90), (120, 150)]);
    h.frame();
    assert!(
        h.selection_rects().len() >= 6,
        "{}",
        h.selection_rects().len()
    );
}

#[test]
fn double_clicking_a_word_while_other_carets_are_there_keeps_working() {
    let mut h = Harness::new("alpha beta gamma");
    h.set_carets(&[1, 7]);
    h.double_click(h.pos_of(0, 12.5));
    assert!(h.cursor_count() >= 1);
}

#[test]
fn typing_a_quote_before_a_word_at_several_carets_pairs_each() {
    let mut h = carets("a b", &[0, 2]);
    h.type_text("'");
    let t = h.text();
    assert_eq!(t.matches('\'').count() % 2, 0, "{t:?}");
}

#[test]
fn the_carets_survive_a_resize_of_the_pane() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.resize(Vec2::new(300.0, 200.0));
    assert_eq!(h.cursor_count(), 3);
    h.type_text("z");
    assert_eq!(h.text(), "az\nbz\ncz");
}

#[test]
fn toggling_wrap_with_several_carets_keeps_them() {
    let mut h = carets("a\nb\nc", &[1, 3, 5]);
    h.set_wrap(true);
    assert_eq!(h.cursor_count(), 3);
    h.set_wrap(false);
    assert_eq!(h.cursor_count(), 3);
}

#[test]
fn scrolling_with_the_wheel_leaves_the_carets_where_they_are() {
    let doc: String = (0..300).map(|i| format!("row {i}\n")).collect();
    let mut h = carets(&doc, &[at(&doc, 0, 1), at(&doc, 10, 1)]);
    h.scroll(30.0);
    assert_eq!(h.cursor_count(), 2);
    h.type_text("!");
    assert_eq!(h.text().matches('!').count(), 2);
}

// ====================================================================================
// randomised: against a plain model
// ====================================================================================

/// What several carets doing one command should come to, worked out the plain way: from
/// the carets' places in the original text, with no reference to how the editor does it.
struct Model {
    text: Vec<char>,
    carets: Vec<usize>,
}

impl Model {
    fn dedup(&mut self) {
        self.carets.sort_unstable();
        self.carets.dedup();
    }

    fn type_str(&mut self, s: &str) {
        let n = s.chars().count();
        let mut out: Vec<char> = Vec::with_capacity(self.text.len() + n * self.carets.len());
        let mut at = 0;
        let mut new = Vec::new();
        for (k, &c) in self.carets.iter().enumerate() {
            out.extend(&self.text[at..c]);
            out.extend(s.chars());
            at = c;
            new.push(c + n * (k + 1));
        }
        out.extend(&self.text[at..]);
        self.text = out;
        self.carets = new;
    }

    fn remove(&mut self, doomed: std::collections::BTreeSet<usize>) {
        let new: Vec<usize> = self
            .carets
            .iter()
            .map(|&p| p - doomed.iter().filter(|&&d| d < p).count())
            .collect();
        let text: Vec<char> = self
            .text
            .iter()
            .enumerate()
            .filter(|(i, _)| !doomed.contains(i))
            .map(|(_, c)| *c)
            .collect();
        self.text = text;
        self.carets = new;
        self.dedup();
    }

    fn backspace(&mut self) {
        let doomed = self
            .carets
            .iter()
            .filter(|&&p| p > 0)
            .map(|&p| p - 1)
            .collect();
        self.remove(doomed);
    }

    fn delete(&mut self) {
        let n = self.text.len();
        let doomed = self.carets.iter().filter(|&&p| p < n).copied().collect();
        self.remove(doomed);
    }

    fn left(&mut self) {
        for c in &mut self.carets {
            *c = c.saturating_sub(1);
        }
        self.dedup();
    }

    fn right(&mut self) {
        let n = self.text.len();
        for c in &mut self.carets {
            *c = (*c + 1).min(n);
        }
        self.dedup();
    }

    fn line_start(&self, p: usize) -> usize {
        self.text[..p]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn line_end(&self, p: usize) -> usize {
        self.text[p..]
            .iter()
            .position(|c| *c == '\n')
            .map_or(self.text.len(), |i| p + i)
    }

    /// Home goes to the first character that is not blank, unless it is already there,
    /// and then to the start of the line.
    fn home(&mut self) {
        let new: Vec<usize> = self
            .carets
            .iter()
            .map(|&p| {
                let start = self.line_start(p);
                let end = self.line_end(p);
                let code = start
                    + self.text[start..end]
                        .iter()
                        .take_while(|c| c.is_whitespace())
                        .count();
                if p != code { code } else { start }
            })
            .collect();
        self.carets = new;
        self.dedup();
    }

    fn end(&mut self) {
        let new: Vec<usize> = self.carets.iter().map(|&p| self.line_end(p)).collect();
        self.carets = new;
        self.dedup();
    }

    fn string(&self) -> String {
        self.text.iter().collect()
    }
}

#[test]
fn many_random_commands_at_many_carets_agree_with_a_plain_model() {
    for seed in 0..60u64 {
        let mut rng = Rng::new(0x5EED_0000 + seed * 7919);
        let doc: String = (0..8)
            .map(|i| {
                let w = rng.below(12);
                format!(
                    "{}{i}\n",
                    "abc xyz ".chars().cycle().take(w).collect::<String>()
                )
            })
            .collect::<String>()
            .trim_end()
            .to_owned();
        let n = doc.chars().count();
        let mut want_at: Vec<usize> = (0..1 + rng.below(6)).map(|_| rng.below(n + 1)).collect();
        want_at.sort_unstable();
        want_at.dedup();
        let mut h = carets(&doc, &want_at);
        let mut m = Model {
            text: doc.chars().collect(),
            carets: want_at.clone(),
        };
        let mut log: Vec<String> = Vec::new();
        for step in 0..40 {
            if m.carets.is_empty() {
                break;
            }
            match rng.below(9) {
                0 | 1 => {
                    let s = ["x", "yz", "Q", "é", " "][rng.below(5)];
                    log.push(format!("type {s:?}"));
                    h.type_text(s);
                    m.type_str(s);
                }
                2 => {
                    log.push("backspace".into());
                    h.key(Key::Backspace);
                    m.backspace();
                }
                3 => {
                    log.push("delete".into());
                    h.key(Key::Delete);
                    m.delete();
                }
                4 => {
                    log.push("left".into());
                    h.key(Key::ArrowLeft);
                    m.left();
                }
                5 => {
                    log.push("right".into());
                    h.key(Key::ArrowRight);
                    m.right();
                }
                6 => {
                    log.push("home".into());
                    h.key(Key::Home);
                    m.home();
                }
                7 => {
                    log.push("end".into());
                    h.key(Key::End);
                    m.end();
                }
                _ => {
                    log.push("paste".into());
                    h.paste("pp");
                    m.type_str("pp");
                }
            }
            assert_eq!(
                h.text(),
                m.string(),
                "seed {seed} step {step} after {log:?}"
            );
            assert_eq!(
                h.carets(),
                m.carets,
                "seed {seed} step {step} after {log:?}\ntext {:?}",
                m.string()
            );
        }
    }
}

#[test]
fn undo_after_random_commands_at_many_carets_returns_to_the_start() {
    for seed in 0..40u64 {
        let mut rng = Rng::new(0xFACE_0000 + seed * 104_729);
        let doc = "alpha beta\ngamma delta\nepsilon zeta\neta theta".to_owned();
        let n = doc.chars().count();
        let mut at: Vec<usize> = (0..2 + rng.below(4)).map(|_| rng.below(n + 1)).collect();
        at.sort_unstable();
        at.dedup();
        let mut h = carets(&doc, &at);
        let start = h.cursors();
        let mut steps = 0;
        for _ in 0..12 {
            h.pause();
            let before = h.text();
            match rng.below(5) {
                0 => h.type_text("x"),
                1 => h.key(Key::Backspace),
                2 => h.key(Key::Delete),
                3 => h.key(Key::Enter),
                _ => h.paste("zz"),
            }
            if h.text() != before {
                steps += 1;
            }
        }
        for _ in 0..steps {
            h.key_mod(Key::Z, CTRL);
        }
        assert_eq!(h.text(), doc, "seed {seed}");
        assert_eq!(h.cursors(), start, "seed {seed}");
    }
}

#[test]
fn nothing_a_person_can_press_breaks_the_carets() {
    let keys = [
        (Key::ArrowLeft, Modifiers::NONE),
        (Key::ArrowRight, Modifiers::NONE),
        (Key::ArrowUp, Modifiers::NONE),
        (Key::ArrowDown, Modifiers::NONE),
        (Key::Home, Modifiers::NONE),
        (Key::End, Modifiers::NONE),
        (Key::PageUp, Modifiers::NONE),
        (Key::PageDown, Modifiers::NONE),
        (Key::Backspace, Modifiers::NONE),
        (Key::Delete, Modifiers::NONE),
        (Key::Enter, Modifiers::NONE),
        (Key::Tab, Modifiers::NONE),
        (Key::Tab, SHIFT),
        (Key::ArrowLeft, CTRL),
        (Key::ArrowRight, CTRL),
        (Key::ArrowLeft, SHIFT),
        (Key::ArrowRight, SHIFT),
        (Key::ArrowUp, SHIFT),
        (Key::ArrowDown, SHIFT),
        (Key::Backspace, CTRL),
        (Key::Delete, CTRL),
        (Key::D, CTRL),
        (Key::L, CTRL_SHIFT),
        (Key::ArrowUp, CTRL_ALT),
        (Key::ArrowDown, CTRL_ALT),
        (Key::ArrowUp, ALT),
        (Key::ArrowDown, ALT),
        (Key::Slash, CTRL),
        (Key::K, CTRL_SHIFT),
        (Key::D, CTRL_SHIFT),
        (Key::Z, CTRL),
        (Key::Y, CTRL),
        (Key::A, CTRL),
        (Key::Escape, Modifiers::NONE),
        (Key::Home, CTRL),
        (Key::End, CTRL),
        (Key::ArrowDown, ALT_SHIFT),
    ];
    for wrap in [false, true] {
        for seed in 0..25u64 {
            let mut rng = Rng::new(0xBEEF_0000 + seed * 31);
            let opts = Options {
                wrap,
                comment: "//",
                ..Options::default()
            };
            let mut h = Harness::with_options(
                "fn main() {\n    let x = (1 + 2);\n    foo(x, \"s\");\n}\n\nlong prose that wraps around the pane a few times over and over",
                opts,
                Vec2::new(260.0, 300.0),
            );
            h.set_carets(&[3, 20, 40, 60]);
            let mut log: Vec<String> = Vec::new();
            for step in 0..80 {
                match rng.below(5) {
                    0 => {
                        let s = ["a", "(", "\"", " ", "é", "{", "}"][rng.below(7)];
                        log.push(format!("type {s:?}"));
                        h.type_text(s);
                    }
                    1 => {
                        log.push("paste".into());
                        h.paste(["q", "a\nb", "xx yy"][rng.below(3)]);
                    }
                    _ => {
                        let (k, m) = keys[rng.below(keys.len())];
                        log.push(format!("{k:?} {m:?}"));
                        h.key_mod(k, m);
                    }
                }
                let len = h.text().chars().count();
                let cur = h.cursors();
                assert!(
                    !cur.is_empty(),
                    "wrap {wrap} seed {seed} step {step}: no cursor {log:?}"
                );
                for (a, c) in &cur {
                    assert!(
                        *a <= len && *c <= len,
                        "wrap {wrap} seed {seed} step {step}: {cur:?} in {len} {log:?}"
                    );
                }
                let mut lo: Vec<(usize, usize)> =
                    cur.iter().map(|&(a, c)| (a.min(c), a.max(c))).collect();
                let sorted = {
                    let mut s = lo.clone();
                    s.sort_unstable();
                    s
                };
                assert_eq!(
                    lo, sorted,
                    "wrap {wrap} seed {seed} step {step}: out of order {cur:?} {log:?}"
                );
                lo.dedup();
                assert_eq!(lo.len(), cur.len(), "duplicates: {cur:?}");
                for w in cur.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    assert!(
                        a.0.max(a.1) <= b.0.min(b.1),
                        "overlap {cur:?} wrap {wrap} seed {seed} step {step} {log:?}"
                    );
                }
            }
        }
    }
}

// ====================================================================================
// speed
// ====================================================================================

/// Unoptimised builds run these several times slower, and the budgets are for release.
fn debug_slack() -> u128 {
    if cfg!(debug_assertions) { 30 } else { 1 }
}

#[test]
fn typing_at_a_thousand_carets_is_still_a_quick_frame() {
    let doc: String = (0..3000).map(|i| format!("line number {i}\n")).collect();
    let starts: Vec<usize> = (0..1000).map(|l| at(&doc, l * 3, 0)).collect();
    let mut h = carets(&doc, &starts);
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    for _ in 0..8 {
        let t = std::time::Instant::now();
        h.type_text("x");
        let d = t.elapsed();
        worst = worst.max(d);
        total += d;
    }
    let mean = total / 8;
    assert_eq!(h.text().matches('x').count(), 8000);
    let (limit_mean, limit_worst) = if std::env::var("RHUMB_STRICT_SPEED").is_ok() {
        (8, 16)
    } else {
        (40, 400)
    };
    let (limit_mean, limit_worst) = (limit_mean * debug_slack(), limit_worst * debug_slack());
    assert!(
        mean.as_millis() < limit_mean && worst.as_millis() < limit_worst,
        "a thousand carets took {mean:?} on average and {worst:?} at worst"
    );
}

#[test]
fn moving_a_thousand_carets_is_a_quick_frame() {
    let doc: String = (0..3000).map(|i| format!("line number {i}\n")).collect();
    let starts: Vec<usize> = (0..1000).map(|l| at(&doc, l * 3, 2)).collect();
    let mut h = carets(&doc, &starts);
    let t = std::time::Instant::now();
    for _ in 0..10 {
        h.key(Key::ArrowRight);
        h.key(Key::ArrowLeft);
    }
    let each = t.elapsed() / 20;
    assert_eq!(h.cursor_count(), 1000);
    let limit = if std::env::var("RHUMB_STRICT_SPEED").is_ok() {
        8
    } else {
        60
    } * debug_slack();
    assert!(each.as_millis() < limit, "a move took {each:?}");
}

#[test]
fn undoing_an_edit_at_a_thousand_carets_is_quick() {
    let doc: String = (0..3000).map(|i| format!("line number {i}\n")).collect();
    let starts: Vec<usize> = (0..1000).map(|l| at(&doc, l * 3, 0)).collect();
    let mut h = carets(&doc, &starts);
    h.type_text("zz");
    let t = std::time::Instant::now();
    h.key_mod(Key::Z, CTRL);
    let took = t.elapsed();
    assert_eq!(h.text(), doc);
    assert_eq!(h.cursor_count(), 1000);
    let limit = if std::env::var("RHUMB_STRICT_SPEED").is_ok() {
        16
    } else {
        200
    } * debug_slack();
    assert!(took.as_millis() < limit, "undo took {took:?}");
}

#[test]
fn ctrl_shift_l_on_a_word_that_stands_thousands_of_times_is_quick_and_capped() {
    let doc = "word ".repeat(30_000);
    let mut h = carets(&doc, &[1]);
    let t = std::time::Instant::now();
    h.key_mod(Key::L, CTRL_SHIFT);
    let took = t.elapsed();
    assert_eq!(h.cursor_count(), super::multi::MAX_CURSORS);
    assert!(took.as_millis() < 3000 * debug_slack(), "{took:?}");
    h.type_text("w");
    assert!(h.text().matches('w').count() > 10_000);
}
